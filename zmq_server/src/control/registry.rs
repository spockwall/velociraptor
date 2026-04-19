//! Per-subscription state and the market-data dispatch fan-out.

use crate::protocol::{Action, SubscriptionKey, SubscriptionRequest, SubscriptionType};
use crate::topics::Topic;
use crate::topics::{bba::BbaTopic, snapshot::SnapshotTopic};
use libs::protocol::OrderbookSnapshot;
use std::collections::HashMap;
use tracing::{error, info, warn};

pub type Registry = HashMap<SubscriptionKey, SubscriptionType>;

/// Apply a subscribe/unsubscribe request to the registry.
pub(super) fn handle_request(
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
                "Client {client_id:?} subscribed {sub_type:?} {}:{}",
                req.exchange, req.symbol
            );
            registry.insert(key, sub_type);
        }
        Action::Unsubscribe => {
            registry.remove(&key);
            info!(
                "Client {:?} unsubscribed {}:{}",
                client_id, req.exchange, req.symbol
            );
        }
    }
}

/// Dispatch a snapshot to every subscriber for this exchange+symbol.
pub fn dispatch(snap: &OrderbookSnapshot, registry: &Registry) -> Vec<(String, Vec<u8>)> {
    let ex = snap.exchange.to_str();
    let sym = &snap.symbol;
    let mut out = Vec::new();

    for (key, sub_type) in registry.iter() {
        if key.exchange != ex || &key.symbol != sym {
            continue;
        }
        let frame = match sub_type {
            SubscriptionType::Snapshot => SnapshotTopic(snap).frame(),
            SubscriptionType::Bba => BbaTopic(snap).frame(),
        };
        match frame {
            Some(f) => out.push(f),
            None => error!("msgpack encode error for {ex}:{sym}"),
        }
    }
    out
}
