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
use libs::redis_client::RedisHandle;
use orderbook::configs::init_logging;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "backend", about = "HTTP API backend — reads Redis, exposes market data")]
struct Args {
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/server.yaml")]
    config: String,

    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    #[arg(long, env = "LOG_JSON", default_value_t = false)]
    log_json: bool,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    init_logging(&args.log_level, args.log_json);

    if let Err(e) = run(&args.config).await {
        tracing::error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(config_path: &str) -> Result<()> {
    let cfg = Config::load(config_path);

    let redis = RedisHandle::connect(&cfg.redis.url, cfg.redis.event_list_cap).await?;
    info!("Redis connected: {}", cfg.redis.url);

    let gamma = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("velociraptor-backend/0.1")
        .build()?;
    let state = Arc::new(AppState { redis, gamma });
    let app = router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.backend.port));
    info!("Backend listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
