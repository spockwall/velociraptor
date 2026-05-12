//! Executor binary entrypoint. Wires REST clients, ZMQ ROUTER gateway,
//! redis control plane, audit sink, metrics, and graceful shutdown.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use executor::control::{drain, run_watcher, ControlState, ShutdownState};
use executor::gateway::idempotency::IdempotencyCache;
use executor::gateway::{ClientMap, Gateway, GatewayConfig};
use executor::ops::{ensure_owner_only, AuditSink, Metrics};
use executor::rest::polymarket::PolymarketRestClient;
use executor::rest::RestOrderClient;
use libs::configs::Config;
use libs::credentials::PolymarketCredentials;
use libs::endpoints::polymarket::polymarket as poly_ep;
use libs::protocol::ExchangeName;
use libs::redis_client::RedisHandle;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Executor CLI. Most settings come from `--config` (the `executor:` section
/// of a Velociraptor YAML config). The flags below either point at that file
/// or override individual settings for ad-hoc runs.
#[derive(Parser, Debug)]
#[command(name = "executor", about = "Velociraptor order executor")]
struct Args {
    /// Path to the unified YAML config (e.g. `configs/example.yaml`). The
    /// `executor:` section drives every default below.
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/example.yaml")]
    config: String,

    /// Path to the credentials file (must contain a `polymarket:` section).
    #[arg(long, default_value = "credentials/polymarket.yaml")]
    credentials: String,

    #[arg(long, env = "REDIS_URL")]
    redis_url: Option<String>,

    /// Override the executor section's `router_endpoint`.
    #[arg(long)]
    router_endpoint: Option<String>,

    /// Override the executor section's `audit_dir`.
    #[arg(long)]
    audit_dir: Option<String>,

    /// Override the executor section's `audit_stream_cap`.
    #[arg(long)]
    audit_stream_cap: Option<usize>,

    /// Override the executor section's `metrics_addr`.
    #[arg(long)]
    metrics_addr: Option<String>,

    /// Override the executor section's `polymarket_env`.
    #[arg(long)]
    polymarket_env: Option<String>,

    /// Skip the credentials file-mode check (useful in dev / docker).
    #[arg(long)]
    skip_chmod_check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_filter = EnvFilter::new("info");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let args = Args::parse();

    // ── Resolve effective settings: file → CLI override ──────────────────
    let cfg = Config::load(&args.config);
    let exec_cfg = &cfg.executor;
    let router_endpoint = args
        .router_endpoint
        .clone()
        .unwrap_or_else(|| exec_cfg.router_endpoint.clone());
    let audit_dir = args
        .audit_dir
        .clone()
        .unwrap_or_else(|| exec_cfg.audit_dir.clone());
    let audit_stream_cap = args.audit_stream_cap.unwrap_or(exec_cfg.audit_stream_cap);
    let metrics_addr_str = args
        .metrics_addr
        .clone()
        .unwrap_or_else(|| exec_cfg.metrics_addr.clone());
    let polymarket_env = args
        .polymarket_env
        .clone()
        .unwrap_or_else(|| exec_cfg.polymarket_env.clone());
    let redis_url = args
        .redis_url
        .clone()
        .unwrap_or_else(|| cfg.redis.url.clone());

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

    // ── Build per-exchange REST clients ──────────────────────────────────
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

    // ── Audit ────────────────────────────────────────────────────────────
    let audit = Arc::new(
        AuditSink::open(PathBuf::from(&audit_dir), redis.clone(), audit_stream_cap).await?,
    );

    // ── Metrics ──────────────────────────────────────────────────────────
    let metrics = Arc::new(Metrics::new());
    let metrics_addr: std::net::SocketAddr = metrics_addr_str.parse()?;
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        if let Err(e) = executor::ops::metrics::serve(metrics_clone, metrics_addr).await {
            warn!("metrics: server error: {e}");
        }
    });

    // ── Control plane ────────────────────────────────────────────────────
    let control = Arc::new(ControlState::default());
    let shutdown = Arc::new(ShutdownState::default());
    let watcher_shutdown = Arc::new(AtomicBool::new(false));

    if let Some(r) = redis.clone() {
        let exchanges: Vec<_> = clients.keys().copied().collect();
        let control_for_watcher = control.clone();
        let watcher_shutdown_clone = watcher_shutdown.clone();
        let clients_for_cancel = clients.clone();
        let audit_for_cancel = audit.clone();
        tokio::spawn(async move {
            run_watcher(
                control_for_watcher,
                r,
                exchanges,
                watcher_shutdown_clone,
                move || {
                    let clients = clients_for_cancel.clone();
                    let audit = audit_for_cancel.clone();
                    async move {
                        for (ex, c) in clients.iter() {
                            match c.cancel_all().await {
                                Ok(n) => {
                                    audit
                                        .log_synthetic(
                                            "cancel_all_fired",
                                            serde_json::json!({
                                                "exchange": ex.to_str(),
                                                "count": n,
                                            }),
                                        )
                                        .await;
                                }
                                Err(e) => {
                                    audit
                                        .log_synthetic(
                                            "cancel_all_error",
                                            serde_json::json!({
                                                "exchange": ex.to_str(),
                                                "error": format!("{e:?}"),
                                            }),
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                },
            )
            .await;
        });
    }

    // ── Gateway ──────────────────────────────────────────────────────────
    let idem = Arc::new(IdempotencyCache::default());
    let gateway = Gateway::new(
        GatewayConfig {
            bind: router_endpoint.clone(),
        },
        Arc::new(clients),
        audit.clone(),
        control.clone(),
        idem,
        shutdown.clone(),
    );

    // ── Run gateway + Ctrl-C handler concurrently ────────────────────────
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
