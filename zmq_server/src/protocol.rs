use crate::trading::events::OrderbookSnapshotPayload;
use crate::types::{Action, SubscriptionType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Inbound (client → server) ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SubscriptionRequest {
    pub action: Action,
    pub exchange: String,
    pub symbol: String,
    #[serde(rename = "type")]
    pub subscription_type: Option<SubscriptionType>,
    pub interval: Option<u64>,
}

// ── Outbound (server → client) ────────────────────────────────────────────────

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
            sub_type: req
                .subscription_type
                .as_ref()
                .map(|t| format!("{t:?}").to_lowercase()),
            interval: req.interval,
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

    pub fn ok_add_channel(exchange: &str, symbol: &str) -> Self {
        Self {
            status: "ok",
            exchange: Some(exchange.to_string()),
            symbol: Some(symbol.to_string()),
            sub_type: None,
            interval: None,
            message: Some("channel added".into()),
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

/// Best-bid-ask only — derived from `OrderbookSnapshotPayload` for `type: "bba"`.
#[derive(Serialize)]
pub struct BbaPayload<'a> {
    pub exchange: &'a str,
    pub symbol: &'a str,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
}

impl<'a> BbaPayload<'a> {
    pub fn from_snapshot(snap: &'a OrderbookSnapshotPayload) -> Self {
        Self {
            exchange: snap.exchange.to_str(),
            symbol: &snap.symbol,
            sequence: snap.sequence,
            timestamp: snap.timestamp,
            best_bid: snap.best_bid,
            best_ask: snap.best_ask,
            spread: snap.spread,
        }
    }
}
