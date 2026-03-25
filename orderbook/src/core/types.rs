use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Delta (canonical wire message) ───────────────────────────────────────────

/// Action carried by every `OrderBookDelta`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeltaAction {
    /// Full book replacement — discard existing state and rebuild.
    Partial,
    /// Incremental price-level upsert/delete (qty == 0.0 means remove).
    Update,
}

/// Canonical orderbook delta — the message passed from the connection pool
/// to the trading engine.  All exchange-specific wire formats convert into
/// this type before reaching `Orderbook::apply_update`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderBookDelta {
    pub exchange: String,
    pub symbol: String,
    pub action: DeltaAction,
    pub timestamp_ms: i64,
    pub sequence: i64,
    /// Sequence of the previous event — used for gap detection.
    /// 0 means "not applicable" (e.g. first event after a reconnect).
    pub prev_sequence: i64,
    /// Keys: `"bids"`, `"asks"`.  Values: `(price, qty)` pairs.
    /// qty == 0.0 signals level removal.
    pub payload: HashMap<String, Vec<(f64, f64)>>,
}

// ── Exchange identity ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeName {
    Binance,
    Other(String),
}

impl std::fmt::Display for ExchangeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExchangeName::Binance => write!(f, "binance"),
            ExchangeName::Other(s) => write!(f, "{}", s),
        }
    }
}

// ── Internal orderbook types ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderSide {
    Bid,
    Ask,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderSide::Bid => write!(f, "bid"),
            OrderSide::Ask => write!(f, "ask"),
        }
    }
}

/// Internal update consumed by `Orderbook::apply_update`.
/// Constructed exclusively via `From<OrderBookDelta>`.
#[derive(Debug, Clone)]
pub struct OrderbookUpdate {
    pub action: DeltaAction,
    pub timestamp: DateTime<Utc>,
    pub sequence: i64,
    pub prev_sequence: i64,
    /// Keys: `"bids"`, `"asks"`.  Values: `(price, qty)` pairs.
    pub payload: HashMap<String, Vec<(f64, f64)>>,
}

#[derive(Clone, Debug)]
pub struct PriceLevel {
    pub price: f64,
    pub total_qty: f64,
}
