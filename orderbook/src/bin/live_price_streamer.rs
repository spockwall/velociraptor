//! Coinbase BTC-USD spot ticker → Redis streamer.
//!
//! Polls `https://api.exchange.coinbase.com/products/{product}/ticker` (public,
//! no auth) on a tight cadence (~1s by default) and:
//!
//! 1. `SET live_price:{product}` → JSON `{ price, ts, source }` for fast fetch.
//! 2. `PUBLISH live_price:{product}` for the frontend's WS bridge.
//!
//! `product` defaults to `BTC-USD`. Run multiple instances for other pairs.
//!
//! Run:
//!   cargo run --bin live_price_streamer --release -- --product BTC-USD

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use libs::redis_client::RedisHandle;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "live_price_streamer",
    about = "Stream Coinbase spot price into Redis for the frontend"
)]
struct Args {
    #[arg(long, default_value = "BTC-USD")]
    product: String,

    #[arg(long, env = "REDIS_URL", default_value = "redis://127.0.0.1:6379")]
    redis_url: String,

    /// Poll interval in milliseconds. Coinbase's public rate limit is generous;
    /// 1000ms keeps us well under it.
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,

    #[arg(long, default_value_t = 4)]
    http_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct CoinbaseTicker {
    /// Last trade price as a string (Coinbase always returns strings for prices).
    price: String,
    /// ISO 8601 timestamp of the last trade.
    time: Option<String>,
}

fn live_price_key(product: &str) -> String {
    format!("live_price:{product}")
}

async fn fetch_one(http: &reqwest::Client, product: &str) -> Result<(f64, Option<String>)> {
    let url = format!("https://api.exchange.coinbase.com/products/{product}/ticker");
    let resp = http.get(&url).send().await?.error_for_status()?;
    let body: CoinbaseTicker = resp.json().await?;
    let price: f64 = body.price.parse().context("parse price")?;
    Ok((price, body.time))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let key = live_price_key(&args.product);

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.http_timeout_secs))
        .user_agent("velociraptor-live-price-streamer/0.1")
        .build()?;
    let redis = RedisHandle::connect(&args.redis_url, 1000).await?;

    info!(
        product = %args.product,
        interval_ms = args.interval_ms,
        key = %key,
        "live_price_streamer: started"
    );

    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        match fetch_one(&http, &args.product).await {
            Ok((price, source_ts)) => {
                let payload = json!({
                    "product": args.product,
                    "price": price,
                    "ts": Utc::now().timestamp_millis(),
                    "source": "coinbase",
                    "source_ts": source_ts,
                })
                .to_string();

                let mut conn = redis.raw();
                if let Err(e) = conn.set::<_, _, ()>(&key, &payload).await {
                    warn!("redis SET {key} failed: {e}");
                }
                if let Err(e) = conn.publish::<_, _, ()>(&key, &payload).await {
                    warn!("redis PUBLISH {key} failed: {e}");
                }
            }
            Err(e) => error!("coinbase fetch failed: {e}"),
        }
    }
}
