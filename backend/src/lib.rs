//! Backend HTTP API — Axum service that reads Redis to expose market data.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use libs::{
    protocol::{BbaPayload, LastTradePrice, OrderbookSnapshot},
    redis_client::{keys::RedisKey, RedisHandle},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct AppState {
    pub redis: RedisHandle,
}

// ── Error type ────────────────────────────────────────────────────────────────

pub enum ApiError {
    NotFound(String),
    Decode(String),
    Redis(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Decode(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            ApiError::Redis(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(ErrorBody { error: msg })).into_response()
    }
}

// ── Query params ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LimitQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true}))
}

async fn get_orderbook(
    State(s): State<Arc<AppState>>,
    Path((exchange, symbol)): Path<(String, String)>,
) -> Result<Json<OrderbookSnapshot>, ApiError> {
    let key = RedisKey::orderbook(&exchange, &symbol);
    let bytes = s
        .redis
        .get_raw(&key)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("no orderbook for {exchange}:{symbol}")))?;
    let snap: OrderbookSnapshot = rmp_serde::from_slice(&bytes)
        .map_err(|e| ApiError::Decode(format!("decode error: {e}")))?;
    Ok(Json(snap))
}

async fn get_bba(
    State(s): State<Arc<AppState>>,
    Path((exchange, symbol)): Path<(String, String)>,
) -> Result<Json<BbaPayload>, ApiError> {
    let key = RedisKey::bba(&exchange, &symbol);
    let bytes = s
        .redis
        .get_raw(&key)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("no BBA for {exchange}:{symbol}")))?;
    let bba: BbaPayload = rmp_serde::from_slice(&bytes)
        .map_err(|e| ApiError::Decode(format!("decode error: {e}")))?;
    Ok(Json(bba))
}

async fn get_snapshots(
    State(s): State<Arc<AppState>>,
    Path((exchange, symbol)): Path<(String, String)>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<OrderbookSnapshot>>, ApiError> {
    let key = RedisKey::snapshots(&exchange, &symbol);
    let stop = (q.limit as isize) - 1;
    let raw_list = s.redis.lrange_raw(&key, 0, stop).await;
    let snaps: Vec<OrderbookSnapshot> = raw_list
        .iter()
        .filter_map(|b| rmp_serde::from_slice(b).ok())
        .collect();
    Ok(Json(snaps))
}

#[derive(Serialize)]
pub struct PolymarketMarket {
    pub asset_id: String,
    pub base_slug: String,
    pub full_slug: String,
    pub side: String,
    pub window_start: u64,
    /// Title formatted like `btc-updown-15m-up-<timestamp>`
    pub title: String,
}

async fn get_polymarket_markets(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<PolymarketMarket>>, ApiError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

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

        // Drop labels whose window has already ended (interval_secs == 0 means
        // a static market — never expires).
        if interval_secs > 0 && window_start > 0 && window_start + interval_secs <= now {
            s.redis.del(&key).await;
            s.redis.srem(RedisKey::POLYMARKET_LABEL_INDEX, &id).await;
            s.redis.del(&RedisKey::orderbook("polymarket", &id)).await;
            s.redis.del(&RedisKey::bba("polymarket", &id)).await;
            s.redis.del(&RedisKey::snapshots("polymarket", &id)).await;
            s.redis.del(&RedisKey::trades("polymarket", &id)).await;
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

#[derive(Serialize)]
pub struct KalshiMarket {
    pub ticker: String,
    pub series: String,
    pub window_start: u64,
    pub interval_secs: u64,
    /// Title — currently the ticker itself; UI can format if desired.
    pub title: String,
}

async fn get_kalshi_markets(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<KalshiMarket>>, ApiError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

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
            s.redis.del(&key).await;
            s.redis.srem(RedisKey::KALSHI_LABEL_INDEX, &ticker).await;
            s.redis.del(&RedisKey::orderbook("kalshi", &ticker)).await;
            s.redis.del(&RedisKey::bba("kalshi", &ticker)).await;
            s.redis.del(&RedisKey::snapshots("kalshi", &ticker)).await;
            s.redis.del(&RedisKey::trades("kalshi", &ticker)).await;
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
    out.sort_by(|a, b| a.series.cmp(&b.series).then(a.window_start.cmp(&b.window_start)).then(a.ticker.cmp(&b.ticker)));
    Ok(Json(out))
}

async fn get_trades(
    State(s): State<Arc<AppState>>,
    Path((exchange, symbol)): Path<(String, String)>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<LastTradePrice>>, ApiError> {
    let key = RedisKey::trades(&exchange, &symbol);
    let stop = (q.limit as isize) - 1;
    let raw_list = s.redis.lrange_raw(&key, 0, stop).await;
    let trades: Vec<LastTradePrice> = raw_list
        .iter()
        .filter_map(|b| rmp_serde::from_slice(b).ok())
        .collect();
    Ok(Json(trades))
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/orderbook/:exchange/:symbol", get(get_orderbook))
        .route("/api/bba/:exchange/:symbol", get(get_bba))
        .route("/api/snapshots/:exchange/:symbol", get(get_snapshots))
        .route("/api/trades/:exchange/:symbol", get(get_trades))
        .route("/api/polymarket/markets", get(get_polymarket_markets))
        .route("/api/kalshi/markets", get(get_kalshi_markets))
        .with_state(state)
}
