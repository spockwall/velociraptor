//! User-event endpoints — fills and order_updates.
//!
//! Reads the capped Redis lists `events:fills` and `events:orders` populated
//! by `zmq_server::setup::attach_redis`. Each list entry is a msgpack-named-map
//! of a `libs::protocol::UserEvent` variant; we decode and re-serialize as JSON
//! so the frontend can consume it without a msgpack decoder.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Json;
use libs::protocol::UserEvent;
use libs::redis_client::keys::Events;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

#[derive(Deserialize)]
pub struct EventsQuery {
    /// Newest-first cap. Defaults to 50, clamped to 500. Backed by `LRANGE 0 limit-1`.
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn fetch_events(s: &AppState, list_key: &str, limit: usize) -> Vec<UserEvent> {
    let raw = s.redis.lrange_raw(list_key, 0, limit as isize - 1).await;
    raw.into_iter()
        .filter_map(|bytes| match rmp_serde::from_slice::<UserEvent>(&bytes) {
            Ok(ev) => Some(ev),
            Err(e) => {
                tracing::warn!(list = list_key, "skipping malformed event: {e}");
                None
            }
        })
        .collect()
}

fn clamp_limit(q: &EventsQuery) -> usize {
    q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub(crate) async fn get_fills(
    State(s): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<UserEvent>>, ApiError> {
    Ok(Json(fetch_events(&s, Events::FILLS, clamp_limit(&q)).await))
}

pub(crate) async fn get_orders(
    State(s): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<UserEvent>>, ApiError> {
    Ok(Json(
        fetch_events(&s, Events::ORDERS, clamp_limit(&q)).await,
    ))
}
