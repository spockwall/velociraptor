use crate::orderbook::OrderbookEngine;
use crate::publisher::protocol::{Ack, BbaPayload, SnapshotPayload, SubscriptionRequest};
use crate::publisher::types::{Action, SubscriptionKey, SubscriptionState, SubscriptionType};
use crate::types::events::{OrderbookEvent, OrderbookSnapshot};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tmq::{Context, Multipart, publish, router};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

type Registry = HashMap<SubscriptionKey, SubscriptionState>;

// ── Publisher ─────────────────────────────────────────────────────────────────

/// Interactive ZMQ publisher.
///
/// - **PUB socket** (`pub_endpoint`, e.g. `tcp://*:5555`): pushes throttled data.
///   Topic format: `"<exchange>:<SYMBOL>"` — e.g. `"binance:BTCUSDT"`.
///
/// - **ROUTER socket** (`router_endpoint`, e.g. `tcp://*:5556`): receives subscription
///   requests from clients (DEALER) and returns acks.
///
/// Subscribes to the engine's broadcast internally.
pub struct ZmqPublisher {
    pub_endpoint: String,
    router_endpoint: String,
    depth: usize,
}

impl ZmqPublisher {
    pub fn new(
        pub_endpoint: impl Into<String>,
        router_endpoint: impl Into<String>,
        depth: usize,
    ) -> Self {
        Self {
            pub_endpoint: pub_endpoint.into(),
            router_endpoint: router_endpoint.into(),
            depth,
        }
    }

    /// Spawn the publisher task. Store the returned handle to keep it alive.
    pub fn start(self, engine: &OrderbookEngine) -> tokio::task::JoinHandle<()> {
        let mut event_rx = engine.subscribe();

        tokio::spawn(async move {
            let ctx = Context::new();
            let Some(mut pub_socket) = bind_pub(&ctx, &self.pub_endpoint) else {
                return;
            };
            let Some(mut router_socket) = bind_router(&ctx, &self.router_endpoint) else {
                return;
            };
            let mut registry = Registry::new();
            let depth = self.depth;

            loop {
                tokio::select! {
                    // ── Control: subscribe / unsubscribe requests ──────────────
                    msg = router_socket.next() => {
                        let Some(Ok(frames)) = msg else { break };
                        if let Some(reply) = handle_control(frames, &mut registry) {
                            if let Err(e) = router_socket.send(reply).await {
                                error!("ZMQ ROUTER send error: {e}");
                            }
                        }
                    }

                    // ── Data: snapshot events from the engine ──────────────────
                    event = event_rx.recv() => match event {
                        Ok(OrderbookEvent::Snapshot(snap)) => {
                            for (topic, json) in dispatch(&snap, &mut registry, depth, Instant::now()) {
                                if let Err(e) = pub_socket.send(vec![topic.into_bytes(), json]).await {
                                    error!("ZMQ PUB send error: {e}");
                                }
                            }
                        }
                        Ok(OrderbookEvent::RawUpdate(_)) => {}
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("ZMQ publisher lagged, skipped {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }

            info!("ZMQ publisher stopped");
        })
    }
}

// ── Socket init ───────────────────────────────────────────────────────────────

fn bind_pub(ctx: &Context, endpoint: &str) -> Option<tmq::publish::Publish> {
    match publish(ctx).bind(endpoint) {
        Ok(s) => {
            info!("ZMQ PUB bound to {endpoint}");
            Some(s)
        }
        Err(e) => {
            error!("ZMQ PUB bind failed: {e}");
            None
        }
    }
}

fn bind_router(ctx: &Context, endpoint: &str) -> Option<tmq::router::Router> {
    match router(ctx).bind(endpoint) {
        Ok(s) => {
            info!("ZMQ ROUTER bound to {endpoint}");
            Some(s)
        }
        Err(e) => {
            error!("ZMQ ROUTER bind failed: {e}");
            None
        }
    }
}

/// Mutate the registry according to the request.
fn apply_request(req: &SubscriptionRequest, client_id: Vec<u8>, registry: &mut Registry) {
    let key = SubscriptionKey {
        client_id: client_id.clone(),
        exchange: req.exchange.clone(),
        symbol: req.symbol.clone(),
    };
    match req.action {
        Action::Subscribe => {
            info!(
                "Client {:?} subscribed {:?} {}:{} @{}ms",
                client_id, req.subscription_type, req.exchange, req.symbol, req.interval
            );
            registry.insert(
                key,
                SubscriptionState {
                    subscription_type: req.subscription_type.clone(),
                    interval: Duration::from_millis(req.interval),
                    last_sent: Instant::now() - Duration::from_millis(req.interval),
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
    }
}

/// Full control-message handling: parse → mutate registry → return reply frames.
fn handle_control(frames: Multipart, registry: &mut Registry) -> Option<Vec<Vec<u8>>> {
    let frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.to_vec()).collect();
    if frames.len() < 3 {
        warn!("ZMQ ROUTER: malformed message ({} frames)", frames.len());
        return None;
    }
    let client_id = frames[0].clone();
    let req = match serde_json::from_slice::<SubscriptionRequest>(&frames[2]) {
        Err(e) => {
            let ack = Ack::error(format!("parse error: {e}"));
            let json = serde_json::to_vec(&ack).ok()?;
            return Some(vec![client_id, vec![], json]);
        }
        Ok(r) => r,
    };
    let ack = match req.action {
        Action::Subscribe => Ack::ok(&req),
        Action::Unsubscribe => Ack::ok_unsub(&req),
    };
    apply_request(&req, client_id.clone(), registry);
    let json = serde_json::to_vec(&ack).ok()?;
    Some(vec![client_id, vec![], json])
}

// ── Data path (PUB) ───────────────────────────────────────────────────────────

/// Build the JSON payload for one subscription type.
fn build_payload(
    snap: &OrderbookSnapshot,
    sub_type: &SubscriptionType,
    depth: usize,
) -> Result<Vec<u8>, serde_json::Error> {
    match sub_type {
        SubscriptionType::Snapshot => {
            serde_json::to_vec(&SnapshotPayload::from_snapshot(snap, depth))
        }
        SubscriptionType::Bba => serde_json::to_vec(&BbaPayload::from_snapshot(snap)),
    }
}

/// Dispatch a snapshot to every matching subscriber that has exceeded its interval.
/// Returns a list of `(topic, payload)` frames ready to send.
fn dispatch(
    snap: &OrderbookSnapshot,
    registry: &mut Registry,
    depth: usize,
    now: Instant,
) -> Vec<(String, Vec<u8>)> {
    let ex = snap.exchange.to_string();
    let sym = &snap.symbol;
    let mut out = Vec::new();

    for (key, state) in registry.iter_mut() {
        if key.exchange != ex || &key.symbol != sym {
            continue;
        }
        if now.duration_since(state.last_sent) < state.interval {
            continue;
        }
        match build_payload(snap, &state.subscription_type, depth) {
            Ok(json) => {
                state.last_sent = now;
                out.push((format!("{ex}:{sym}"), json));
            }
            Err(e) => error!("JSON serialize error: {e}"),
        }
    }
    out
}
