//! Market discovery endpoints. Each scans an exchange's label-index set in
//! Redis, drops entries whose window has already ended (along with their
//! associated orderbook/bba/snapshot/trade data), and returns what's left.

use std::sync::Arc;

use axum::{extract::State, response::Json};
use libs::redis_client::{keys::RedisKey, RedisHandle};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct PolymarketMarket {
    pub asset_id: String,
    pub base_slug: String,
    pub full_slug: String,
    pub side: String,
    pub window_start: u64,
    /// Window length in seconds — needed by the UI to look up the previous
    /// window's `finalPrice` (which equals this window's `priceToBeat` for
    /// chained Up/Down markets) while the current window is still open.
    pub interval_secs: u64,
    /// Title formatted like `btc-updown-15m-up-<timestamp>`
    pub title: String,
}

#[derive(Serialize)]
pub struct KalshiMarket {
    pub ticker: String,
    pub series: String,
    pub window_start: u64,
    pub interval_secs: u64,
    /// Title — currently the ticker itself; UI can format if desired.
    pub title: String,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Tear down all per-asset/per-ticker keys for a market that's no longer
/// active. Called when the GC pass detects an expired window.
async fn expire_label(
    redis: &RedisHandle,
    label_key: &str,
    label_index: &str,
    exchange: &str,
    asset_or_ticker: &str,
) {
    redis.del(label_key).await;
    redis.srem(label_index, asset_or_ticker).await;
    redis.del(&RedisKey::orderbook(exchange, asset_or_ticker)).await;
    redis.del(&RedisKey::bba(exchange, asset_or_ticker)).await;
    redis.del(&RedisKey::snapshots(exchange, asset_or_ticker)).await;
    redis.del(&RedisKey::trades(exchange, asset_or_ticker)).await;
}

pub(crate) async fn get_polymarket_markets(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<PolymarketMarket>>, ApiError> {
    let now = now_secs();

    let ids = s.redis.smembers(RedisKey::POLYMARKET_LABEL_INDEX).await;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let key = RedisKey::polymarket_label(&id);
        let h = s.redis.hgetall(&key).await;
        if h.is_empty() {
            // Orphaned index entry — clean it up.
            s.redis.srem(RedisKey::POLYMARKET_LABEL_INDEX, &id).await;
            continue;
        }
        let window_start = h
            .get("window_start")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let interval_secs = h
            .get("interval_secs")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        // Drop labels whose window has already ended (interval_secs == 0
        // means a static market — never expires).
        if interval_secs > 0 && window_start > 0 && window_start + interval_secs <= now {
            expire_label(
                &s.redis,
                &key,
                RedisKey::POLYMARKET_LABEL_INDEX,
                "polymarket",
                &id,
            )
            .await;
            continue;
        }

        let base_slug = h.get("base_slug").cloned().unwrap_or_default();
        let full_slug = h.get("full_slug").cloned().unwrap_or_default();
        let side = h.get("side").cloned().unwrap_or_default();
        // full_slug already ends with the window timestamp, so just append the side.
        let title = if full_slug.is_empty() {
            format!("{base_slug}-{side}")
        } else {
            format!("{full_slug}-{side}")
        };
        out.push(PolymarketMarket {
            asset_id: id,
            base_slug,
            full_slug,
            side,
            window_start,
            interval_secs,
            title,
        });
    }
    out.sort_by(|a, b| {
        a.base_slug
            .cmp(&b.base_slug)
            .then(a.window_start.cmp(&b.window_start))
            .then(a.side.cmp(&b.side))
    });
    Ok(Json(out))
}

pub(crate) async fn get_kalshi_markets(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<KalshiMarket>>, ApiError> {
    let now = now_secs();

    let tickers = s.redis.smembers(RedisKey::KALSHI_LABEL_INDEX).await;
    let mut out = Vec::with_capacity(tickers.len());
    for ticker in tickers {
        let key = RedisKey::kalshi_label(&ticker);
        let h = s.redis.hgetall(&key).await;
        if h.is_empty() {
            s.redis.srem(RedisKey::KALSHI_LABEL_INDEX, &ticker).await;
            continue;
        }
        let window_start = h
            .get("window_start")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let interval_secs = h
            .get("interval_secs")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        if interval_secs > 0 && window_start > 0 && window_start + interval_secs <= now {
            expire_label(
                &s.redis,
                &key,
                RedisKey::KALSHI_LABEL_INDEX,
                "kalshi",
                &ticker,
            )
            .await;
            continue;
        }

        let series = h.get("series").cloned().unwrap_or_default();
        let title = ticker.clone();
        out.push(KalshiMarket {
            ticker,
            series,
            window_start,
            interval_secs,
            title,
        });
    }
    out.sort_by(|a, b| {
        a.series
            .cmp(&b.series)
            .then(a.window_start.cmp(&b.window_start))
            .then(a.ticker.cmp(&b.ticker))
    });
    Ok(Json(out))
}
