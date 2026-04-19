use serde::{Deserialize, Serialize};

// ── Subscription / Unsubscription definition ─────────────────────────────────────────────────
/// The data type a client subscribes to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionType {
    Snapshot,
    Bba,
}

/// Uniquely identifies one client's subscription to one symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionKey {
    pub client_id: Vec<u8>,
    pub exchange: String,
    pub symbol: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Subscribe,
    Unsubscribe,
}

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
