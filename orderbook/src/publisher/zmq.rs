use crate::orderbook::OrderbookEngine;
use crate::publisher::protocol::{Ack, BbaPayload, SnapshotPayload, SubscriptionRequest};
use crate::publisher::types::{
    Action, ChannelRequest, SubscriptionKey, SubscriptionState, SubscriptionType,
};
use crate::types::events::{OrderbookEvent, OrderbookSnapshot};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tmq::{Context, Multipart, publish, router};
use tokio::sync::{broadcast, mpsc};
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
///   Supported actions: `subscribe`, `unsubscribe`, `add_channel`.
///
/// Subscribes to the engine's broadcast internally.
pub struct ZmqPublisher {
    pub_endpoint: String,
    router_endpoint: String,
    depth: usize,
    /// Sender used to forward `add_channel` requests back to `OrderbookSystem`.
    channel_tx: mpsc::UnboundedSender<ChannelRequest>,
}

impl ZmqPublisher {
    pub fn new(
        pub_endpoint: impl Into<String>,
        router_endpoint: impl Into<String>,
        depth: usize,
        channel_tx: mpsc::UnboundedSender<ChannelRequest>,
    ) -> Self {
        Self {
            pub_endpoint: pub_endpoint.into(),
            router_endpoint: router_endpoint.into(),
            depth,
            channel_tx,
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
            let channel_tx = self.channel_tx;

            loop {
                tokio::select! {
                    // ── Control: subscribe / unsubscribe / add_channel ─────────
                    msg = router_socket.next() => {
                        let Some(Ok(frames)) = msg else { break };
                        if let Some(reply) = handle_control(frames, &mut registry, &channel_tx) {
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
/// Returns an ack for subscribe/unsubscribe; for add_channel the ack is built by handle_control.
fn apply_request(req: &SubscriptionRequest, client_id: Vec<u8>, registry: &mut Registry) {
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
            // Registry not mutated for add_channel — handled in handle_control.
        }
    }
}

/// Full control-message handling: parse → mutate registry → return reply frames.
fn handle_control(
    frames: Multipart,
    registry: &mut Registry,
    channel_tx: &mpsc::UnboundedSender<ChannelRequest>,
) -> Option<Vec<Vec<u8>>> {
    let frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.to_vec()).collect();
    // ROUTER prepends the client identity frame.
    // With tmq, DEALER→ROUTER arrives as [client_id, payload] (2 frames).
    // Some clients insert an empty delimiter, yielding [client_id, "", payload] (3 frames).
    // Support both layouts.
    let has_delimiter = frames.len() >= 3;
    let (client_id, payload) = if has_delimiter {
        (frames[0].clone(), &frames[2])
    } else if frames.len() == 2 {
        (frames[0].clone(), &frames[1])
    } else {
        warn!("ZMQ ROUTER: malformed message ({} frames)", frames.len());
        return None;
    };

    // Build a reply with the same framing the client used.
    let make_reply = |json: Vec<u8>| -> Vec<Vec<u8>> {
        if has_delimiter {
            vec![client_id.clone(), vec![], json]
        } else {
            vec![client_id.clone(), json]
        }
    };

    let req = match serde_json::from_slice::<SubscriptionRequest>(payload) {
        Err(e) => {
            let ack = Ack::error(format!("parse error: {e}"));
            let json = serde_json::to_vec(&ack).ok()?;
            return Some(make_reply(json));
        }
        Ok(r) => r,
    };

    let ack = match req.action {
        Action::Subscribe => {
            apply_request(&req, client_id.clone(), registry);
            Ack::ok(&req)
        }
        Action::Unsubscribe => {
            apply_request(&req, client_id.clone(), registry);
            Ack::ok_unsub(&req)
        }
        Action::AddChannel => {
            info!(
                "Client {:?} requesting new channel {}:{}",
                client_id, req.exchange, req.symbol
            );
            let cr = ChannelRequest {
                client_id: client_id.clone(),
                exchange: req.exchange.clone(),
                symbol: req.symbol.clone(),
            };
            if let Err(e) = channel_tx.send(cr) {
                error!("Failed to forward add_channel request: {e}");
                Ack::error("internal error: channel request dropped")
            } else {
                Ack::ok_add_channel(&req.exchange, &req.symbol)
            }
        }
    };

    let json = serde_json::to_vec(&ack).ok()?;
    Some(make_reply(json))
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
