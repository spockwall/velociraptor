//! Backend binary — Axum HTTP server reading from Redis.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin backend -- --config configs/server.yaml
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
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/server.yaml")]
    config: String,
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

    if let Err(e) = run(cfg).await {
        tracing::error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cfg: Config) -> Result<()> {
    let redis = RedisHandle::connect(&cfg.redis.url, cfg.redis.event_list_cap).await?;
    info!("Redis connected: {}", cfg.redis.url);

    let gamma = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("velociraptor-backend/0.1")
        .build()?;
    let state = Arc::new(AppState { redis, gamma });
    // Per-route-group HTTP tracing is configured in `router()` (orderbook
    // reads at DEBUG, everything else at INFO; 5xx always ERROR).
    let app = router(state).layer(CorsLayer::permissive());

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.backend.port));
    info!("Backend listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
