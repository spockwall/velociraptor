//! Shared wire protocol between the Rust services (orderbook-server, executor)
//! and the Python trading engine.
//!
//! Payloads are encoded with `rmp-serde` (msgpack). The Python side mirrors
//! these structures via `msgpack` + `pydantic`. See `docs/protocol.md`.
//!
//! Market-data payloads (`SnapshotPayload`, `BbaPayload`) live in the
//! `orderbook` crate and are serialized over ZMQ PUB by `ZmqPublisher` — they
//! are not re-exported here to avoid a circular dep on `orderbook`.

pub mod control;
pub mod events;
pub mod orders;

use chrono::{DateTime, Utc};
pub use control::ControlMessage;
use core::fmt;
pub use events::{EventKind, UserEvent};
pub use orders::{
    HeartbeatAck, OrderAck, OrderAction, OrderError, OrderKind, OrderRequest, OrderResponse,
    OrderResult, OrderStatus, PlaceOne, Side, Tif,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeName {
    Okx,
    Binance,
    Polymarket,
    Hyperliquid,
    Kalshi,
}

impl ExchangeName {
    pub fn to_str(&self) -> &'static str {
        match self {
            ExchangeName::Okx => "okx",
            ExchangeName::Binance => "binance",
            ExchangeName::Polymarket => "polymarket",
            ExchangeName::Hyperliquid => "hyperliquid",
            ExchangeName::Kalshi => "kalshi",
        }
    }

    pub fn to_string(&self) -> String {
        self.to_str().to_string()
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "okx" => Some(ExchangeName::Okx),
            "binance" => Some(ExchangeName::Binance),
            "polymarket" => Some(ExchangeName::Polymarket),
            "hyperliquid" => Some(ExchangeName::Hyperliquid),
            "kalshi" => Some(ExchangeName::Kalshi),
            _ => None,
        }
    }
}

impl fmt::Display for ExchangeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

/// A (price, quantity) pair.
pub type PriceLevelTuple = (f64, f64);

/// Materialized orderbook snapshot broadcast to subscribers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderbookSnapshot {
    pub exchange: ExchangeName,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub best_bid: Option<PriceLevelTuple>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}
