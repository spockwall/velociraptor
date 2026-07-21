//! Backend binary — Axum HTTP server reading from Redis.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin backend -- --config configs/dev/config.yaml
//! cargo run --bin backend -- --config configs/prod/config.yaml
//! ```

use anyhow::Result;
use backend::{router, AppState};
use clap::Parser;
use libs::configs::Config;
use libs::logging::init_logging;
use libs::redis_client::RedisHandle;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "backend", about = "HTTP API backend — reads Redis, exposes market data")]
struct Args {
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/dev/config.yaml")]
    config: String,

    /// Credentials file with a `polymarket:` section. Only used to learn the
    /// proxy wallet (`funder`, falling back to `address`) for the auto-redeem
    /// watcher; absent file/section just means no wallet is auto-added.
    #[arg(
        long,
        env = "POLYMARKET_CREDENTIALS_FILE",
        default_value = "credentials/dev/polymarket.yaml"
    )]
    polymarket_credentials: String,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    // Load config first so `logging:` settings can drive tracing setup.
    let cfg = Config::load(&args.config);
    let _guards = init_logging(
        "backend",
        std::path::Path::new(&cfg.logging.dir),
        &cfg.logging.level,
        cfg.logging.json,
    );

    if let Err(e) = run(cfg, &args).await {
        tracing::error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

/// Wallets the auto-redeem watcher polls: `backend.redeem_watch.wallets`
/// from the config plus the credentials wallet (`funder` — the proxy wallet
/// the data-api keys positions by — falling back to `address`), lowercased
/// and deduped.
fn resolve_redeem_wallets(cfg: &Config, creds_path: &str) -> Vec<String> {
    let mut wallets: Vec<String> = cfg
        .backend
        .redeem_watch
        .wallets
        .iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if let Some(creds) = libs::credentials::PolymarketCredentials::try_load(creds_path) {
        let w = creds
            .funder
            .filter(|f| !f.trim().is_empty())
            .unwrap_or(creds.address);
        let w = w.trim().to_lowercase();
        if !w.is_empty() {
            wallets.push(w);
        }
    }
    wallets.sort();
    wallets.dedup();
    wallets
}

async fn run(cfg: Config, args: &Args) -> Result<()> {
    let redis = RedisHandle::connect(&cfg.redis.url, cfg.redis.event_list_cap).await?;
    info!("Redis connected: {}", cfg.redis.url);

    // Background system-monitor sampler: periodically snapshots host CPU/mem/
    // disk + systemd status into a capped Redis list and a daily JSON-lines
    // disk log under `{logging.dir}/system/`. Runs for the life of the process.
    tokio::spawn(backend::routes::monitor::sample_loop(
        redis.clone(),
        std::path::PathBuf::from(&cfg.logging.dir),
        std::path::PathBuf::from(&cfg.backend.data_dir),
    ));

    // Background error-log tailer: republishes new lines from every service's
    // daily `{logging.dir}/{service}/{day}.error.log` onto a capped Redis list,
    // caching its per-service read cursor in Redis so restarts resume in place
    // and daily rotation is handled. Read by `GET /api/logs/errors`.
    tokio::spawn(backend::routes::logs::tail_loop(
        redis.clone(),
        std::path::PathBuf::from(&cfg.logging.dir),
    ));

    let gamma = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("velociraptor-backend/0.1")
        .build()?;

    // Background Polymarket auto-redeem watcher: polls the public data-api
    // for `redeemable` positions on the configured wallet(s) and alerts when
    // a winning position stays unredeemed past the threshold (auto-redeem
    // presumed broken). Status lands in Redis, read by
    // `GET /api/pm/redeem-status`; stuck positions ERROR-log so the frontend
    // Logs page shows them.
    if cfg.backend.redeem_watch.enabled {
        let wallets = resolve_redeem_wallets(&cfg, &args.polymarket_credentials);
        if wallets.is_empty() {
            info!(
                creds = %args.polymarket_credentials,
                "redeem watcher: no wallets (no credentials, empty backend.redeem_watch.wallets) — not started"
            );
        } else {
            tokio::spawn(backend::routes::redeem::watch_loop(
                redis.clone(),
                gamma.clone(),
                cfg.backend.redeem_watch.clone(),
                wallets,
            ));
        }
    }

    let state = Arc::new(AppState {
        redis,
        gamma,
        data_dir: std::path::PathBuf::from(&cfg.backend.data_dir),
    });
    // Per-route-group HTTP tracing is configured in `router()` (orderbook
    // reads at DEBUG, everything else at INFO; 5xx always ERROR).
    let app = router(state).layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.backend.port));
    info!("Backend listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
