use crate::orderbook::Orderbook;
use crate::publisher::types::{Action, SubscriptionType};
use crate::types::events::OrderbookSnapshot;
use serde::{Deserialize, Serialize};

// ── Inbound (client → server) ─────────────────────────────────────────────────

/// Subscription request sent by a client over the DEALER socket.
#[derive(Debug, Deserialize)]
pub struct SubscriptionRequest {
    pub action: Action,
    pub exchange: String,
    pub symbol: String,
    #[serde(rename = "type")]
    pub subscription_type: SubscriptionType,
    pub interval: u64, // milliseconds
}

// ── Outbound (server → client) ────────────────────────────────────────────────

/// Acknowledgement sent back to the client after a subscribe/unsubscribe.
#[derive(Debug, Serialize)]
pub struct Ack {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub sub_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Ack {
    pub fn ok(req: &SubscriptionRequest) -> Self {
        Self {
            status: "ok",
            exchange: Some(req.exchange.clone()),
            symbol: Some(req.symbol.clone()),
            sub_type: Some(format!("{:?}", req.subscription_type).to_lowercase()),
            interval: Some(req.interval),
            message: None,
        }
    }

    pub fn ok_unsub(req: &SubscriptionRequest) -> Self {
        Self {
            status: "ok",
            exchange: Some(req.exchange.clone()),
            symbol: Some(req.symbol.clone()),
            sub_type: None,
            interval: None,
            message: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            status: "error",
            exchange: None,
            symbol: None,
            sub_type: None,
            interval: None,
            message: Some(msg.into()),
        }
    }
}

// ── Data payloads (published over PUB socket) ─────────────────────────────────

/// Full depth snapshot — published for `type: "snapshot"` subscriptions.
#[derive(Serialize)]
pub struct SnapshotPayload {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: String,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

/// Best-bid-ask only — published for `type: "bba"` subscriptions.
#[derive(Serialize)]
pub struct BbaPayload {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: String,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
}

impl SnapshotPayload {
    pub fn from_snapshot(snap: &OrderbookSnapshot, depth: usize) -> Self {
        let book: &Orderbook = &snap.book;
        let (bids, asks) = book.depth(depth);
        Self {
            exchange: snap.exchange.to_string(),
            symbol: snap.symbol.clone(),
            sequence: snap.sequence,
            timestamp: snap.timestamp.to_rfc3339(),
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
            spread: book.spread(),
            mid: book.mid_price(),
            wmid: book.wmid(),
            bids,
            asks,
        }
    }
}

impl BbaPayload {
    pub fn from_snapshot(snap: &OrderbookSnapshot) -> Self {
        let book: &Orderbook = &snap.book;
        Self {
            exchange: snap.exchange.to_string(),
            symbol: snap.symbol.clone(),
            sequence: snap.sequence,
            timestamp: snap.timestamp.to_rfc3339(),
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
            spread: book.spread(),
        }
    }
}
