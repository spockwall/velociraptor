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
        .with_state(state)
}
