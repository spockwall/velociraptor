//! Executor binary entrypoint. Builds the `Executor` orchestrator, wires
//! the ZMQ gateway + redis-backed control thread + metrics server, runs
//! boot-time reconcile, then drains gracefully on SIGINT.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use executor::control::{drain, run_watcher, ControlState, ShutdownState};
use executor::gateway::{ClientMap, Gateway, GatewayConfig};
use executor::ops::{ensure_owner_only, AuditSink, Metrics};
use executor::rest::polymarket::PolymarketRestClient;
use executor::rest::RestOrderClient;
use executor::{Executor, ExecutorBuild, ExecutorControlCallbacks};
use libs::configs::Config;
use libs::credentials::PolymarketCredentials;
use libs::endpoints::polymarket::polymarket as poly_ep;
use libs::logging::init_logging;
use libs::protocol::ExchangeName;
use libs::redis_client::RedisHandle;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "executor", about = "Velociraptor order executor")]
struct Args {
    /// Path to the unified YAML config (e.g. `configs/example.yaml`).
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/example.yaml")]
    config: String,

    /// Path to the credentials file (must contain a `polymarket:` section).
    #[arg(long, default_value = "credentials/polymarket.yaml")]
    credentials: String,

    /// Skip the credentials file-mode check (useful in dev / docker).
    #[arg(long)]
    skip_chmod_check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let cfg = Config::load(&args.config);
    let _guards = init_logging(
        "executor",
        std::path::Path::new(&cfg.logging.dir),
        &cfg.logging.level,
        cfg.logging.json,
    );

    let exec_cfg = &cfg.executor;
    let router_endpoint = exec_cfg.router_endpoint.clone();
    let audit_dir = exec_cfg.audit_dir.clone();
    let audit_stream_cap = exec_cfg.audit_stream_cap;
    let metrics_addr_str = exec_cfg.metrics_addr.clone();
    let polymarket_env = exec_cfg.polymarket_env.clone();
    let redis_url = cfg.redis.url.clone();

    info!(
        config = %args.config,
        credentials = %args.credentials,
        router = %router_endpoint,
        metrics = %metrics_addr_str,
        polymarket_env = %polymarket_env,
        "executor: effective settings"
    );

    if !args.skip_chmod_check {
        if let Err(e) = ensure_owner_only(&args.credentials) {
            anyhow::bail!("credentials: {e}");
        }
    }

    // ── REST clients ─────────────────────────────────────────────────────
    let mut clients: ClientMap = HashMap::new();
    let poly_creds = PolymarketCredentials::load(&args.credentials);
    let poly_base = match polymarket_env.as_str() {
        "testnet" | "preprod" | "amoy" => poly_ep::TESTNET_BASE_URL,
        _ => poly_ep::BASE_URL,
    };
    match PolymarketRestClient::new(poly_creds, poly_base).await {
        Ok(c) => {
            clients.insert(
                ExchangeName::Polymarket,
                Arc::new(c) as Arc<dyn RestOrderClient>,
            );
            info!("polymarket: REST client built ({})", poly_base);
        }
        Err(e) => {
            warn!("polymarket: REST client init failed: {e}");
            anyhow::bail!("polymarket REST client could not be initialised");
        }
    }

    // ── Redis ────────────────────────────────────────────────────────────
    let redis = match RedisHandle::connect(&redis_url, 10_000).await {
        Ok(h) => Some(h),
        Err(e) => {
            warn!("redis: connect failed ({e}); audit stream + control plane disabled");
            None
        }
    };

    // ── Audit + metrics ──────────────────────────────────────────────────
    let audit_dir_path = PathBuf::from(&audit_dir);
    let audit = Arc::new(
        AuditSink::open(audit_dir_path.clone(), redis.clone(), audit_stream_cap).await?,
    );
    let metrics = Arc::new(Metrics::new());
    let metrics_addr: std::net::SocketAddr = metrics_addr_str.parse()?;
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        if let Err(e) = executor::ops::metrics::serve(metrics_clone, metrics_addr).await {
            warn!("metrics: server error: {e}");
        }
    });

    // ── Control + shutdown state ─────────────────────────────────────────
    let control = Arc::new(ControlState::default());
    let shutdown = Arc::new(ShutdownState::default());
    let watcher_shutdown = Arc::new(AtomicBool::new(false));

    // ── Build the Executor orchestrator ──────────────────────────────────
    let executor = Arc::new(Executor::new(ExecutorBuild {
        clients,
        audit: audit.clone(),
        metrics: metrics.clone(),
        control: control.clone(),
        shutdown: shutdown.clone(),
        redis: redis.clone(),
        config_path: PathBuf::from(&args.config),
    }));

    // Boot-time registry rehydrate + per-exchange reconcile.
    executor.rehydrate_registry(&audit_dir_path).await;
    for ex in executor.exchanges() {
        if let Some(client) = executor.clients().get(&ex) {
            let (_unknown, _lost) =
                executor::ops::reconcile::reconcile_one(audit.as_ref(), ex, client.clone(), executor.registry())
                    .await;
        }
    }

    // ── Control watcher (only when Redis is up) ──────────────────────────
    if let Some(r) = redis.clone() {
        let exchanges: Vec<_> = executor.exchanges();
        let control_for_watcher = control.clone();
        let watcher_shutdown_clone = watcher_shutdown.clone();
        let callbacks: Arc<dyn executor::control::ControlCallbacks> =
            Arc::new(ExecutorControlCallbacks {
                executor: executor.clone(),
            });
        tokio::spawn(async move {
            run_watcher(
                control_for_watcher,
                r,
                exchanges,
                watcher_shutdown_clone,
                callbacks,
            )
            .await;
        });
    }

    // ── Gateway: ZMQ ROUTER transport ────────────────────────────────────
    let gateway = Gateway::new(
        GatewayConfig {
            bind: router_endpoint.clone(),
        },
        executor.clone(),
    );

    let gateway_handle = tokio::spawn(async move { gateway.run().await });

    tokio::signal::ctrl_c().await?;
    info!("shutdown: SIGINT received");
    watcher_shutdown.store(true, Ordering::Relaxed);
    let unresolved = drain(shutdown, audit, Duration::from_secs(5)).await;
    let _ = gateway_handle.abort();

    if unresolved > 0 {
        std::process::exit(1);
    }
    Ok(())
}
