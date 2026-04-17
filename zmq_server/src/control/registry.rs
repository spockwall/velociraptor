//! Per-subscription state and the market-data dispatch fan-out.

use crate::frame::market::{build_payload, topic_for};
use crate::protocol::SubscriptionRequest;
use crate::trading::events::OrderbookSnapshotPayload;
use crate::types::{Action, SubscriptionKey, SubscriptionState, SubscriptionType};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

pub type Registry = HashMap<SubscriptionKey, SubscriptionState>;

/// Apply a subscribe/unsubscribe request to the registry.
pub(super) fn apply_request(req: &SubscriptionRequest, client_id: Vec<u8>, registry: &mut Registry) {
    let key = SubscriptionKey {
        client_id: client_id.clone(),
        exchange: req.exchange.clone(),
        symbol: req.symbol.clone(),
    };
    match req.action {
        Action::Subscribe => {
            let sub_type = match &req.subscription_type {
                Some(t) => t.clone(),
                None => {
                    warn!("Subscribe request missing type, defaulting to Snapshot");
                    SubscriptionType::Snapshot
                }
            };
            let interval_ms = req.interval.unwrap_or(500);
            info!(
                "Client {:?} subscribed {:?} {}:{} @{}ms",
                client_id, sub_type, req.exchange, req.symbol, interval_ms
            );
            registry.insert(
                key,
                SubscriptionState {
                    subscription_type: sub_type,
                    interval: Duration::from_millis(interval_ms),
                    last_sent: Instant::now() - Duration::from_millis(interval_ms),
                },
            );
        }
        Action::Unsubscribe => {
            registry.remove(&key);
            info!(
                "Client {:?} unsubscribed {}:{}",
                client_id, req.exchange, req.symbol
            );
        }
        Action::AddChannel => {
            // Registry not mutated; handled in handle_control.
        }
    }
}

/// Dispatch a snapshot to every matching subscriber whose interval has elapsed.
pub fn dispatch(
    snap: &OrderbookSnapshotPayload,
    registry: &mut Registry,
    now: Instant,
) -> Vec<(String, Vec<u8>)> {
    let ex = snap.exchange.to_str();
    let sym = &snap.symbol;
    let mut out = Vec::new();

    for (key, state) in registry.iter_mut() {
        if key.exchange != ex || &key.symbol != sym {
            continue;
        }
        if now.duration_since(state.last_sent) < state.interval {
            continue;
        }
        match build_payload(snap, &state.subscription_type) {
            Ok(bytes) => {
                state.last_sent = now;
                out.push((topic_for(ex, sym), bytes));
            }
            Err(e) => error!("msgpack serialize error: {e}"),
        }
    }
    out
}
