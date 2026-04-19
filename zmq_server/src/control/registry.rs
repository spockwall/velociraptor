//! Per-subscription state and the market-data dispatch fan-out.

use crate::frame::market::{build_payload, topic_for};
use crate::protocol::SubscriptionRequest;
use crate::types::{Action, SubscriptionKey, SubscriptionState, SubscriptionType};
use orderbook::OrderbookSnapshot;
use std::collections::HashMap;
use tracing::{error, info, warn};

pub type Registry = HashMap<SubscriptionKey, SubscriptionState>;

/// Apply a subscribe/unsubscribe request to the registry.
pub(super) fn apply_request(
    req: &SubscriptionRequest,
    client_id: Vec<u8>,
    registry: &mut Registry,
) {
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
            info!(
                "Client {:?} subscribed {:?} {}:{}",
                client_id, sub_type, req.exchange, req.symbol
            );
            registry.insert(key, SubscriptionState { subscription_type: sub_type });
        }
        Action::Unsubscribe => {
            registry.remove(&key);
            info!("Client {:?} unsubscribed {}:{}", client_id, req.exchange, req.symbol);
        }
    }
}

/// Dispatch a snapshot to every subscriber for this exchange+symbol.
pub fn dispatch(snap: &OrderbookSnapshot, registry: &Registry) -> Vec<(String, Vec<u8>)> {
    let ex = snap.exchange.to_str();
    let sym = &snap.symbol;
    let mut out = Vec::new();

    for (key, state) in registry.iter() {
        if key.exchange != ex || &key.symbol != sym {
            continue;
        }
        match build_payload(snap, &state.subscription_type) {
            Ok(bytes) => out.push((topic_for(ex, sym), bytes)),
            Err(e) => error!("msgpack serialize error: {e}"),
        }
    }
    out
}
