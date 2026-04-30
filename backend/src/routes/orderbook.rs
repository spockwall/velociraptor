//! Handlers that read raw msgpack values out of Redis and return JSON:
//! `/health`, `/api/orderbook`, `/api/bba`, `/api/snapshots`, `/api/trades`.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use libs::{
    protocol::{BbaPayload, LastTradePrice, OrderbookSnapshot},
    redis_client::keys::RedisKey,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub(crate) struct LimitQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

pub(crate) async fn get_orderbook(
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

pub(crate) async fn get_bba(
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

pub(crate) async fn get_snapshots(
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

pub(crate) async fn get_trades(
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
