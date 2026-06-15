//! Shared wire protocol between Rust services and the Python trading engine.
//!
//! All payloads are msgpack (`rmp-serde`). The Python side mirrors these
//! structs via `msgpack` + `pydantic`. See `docs/protocol.md` and the ZMQ
//! Interface section in `README.md` for socket layout and topic formats.
//!
//! ## Module layout
//!
//! | Module | Contents |
//! |---|---|
//! | `events` | `UserEvent` (fill, order_update, balance, position) + `EventKind` |
//! | `orders` | `OrderRequest`, `OrderResponse`, `OrderAction`, `OrderResult`, `OrderError` |
//! | `control` | `ControlMessage` (shutdown, pause, resume, strategy_params) |
//!
//! `OrderbookSnapshot` lives here too — it is the market-data payload
//! serialized on `MARKET_DATA_SOCKET` by `zmq_server::topic::market`.

pub mod control;
pub mod events;
pub mod orders;

use chrono::{DateTime, Utc};
pub use control::ControlMessage;
use core::fmt;
pub use events::{BbaPayload, EventKind, UserEvent};
pub use orders::{
    FillInfo, HeartbeatAck, OrderAck, OrderAction, OrderError, OrderKind, OrderMeta, OrderRequest,
    OrderResponse, OrderResult, OrderStatus, PlaceOne, Side, Tif,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeName {
    Okx,
    Binance,
    BinanceSpot,
    Polymarket,
    Hyperliquid,
    Kalshi,
}

impl ExchangeName {
    pub fn to_str(&self) -> &'static str {
        match self {
            ExchangeName::Okx => "okx",
            ExchangeName::Binance => "binance",
            ExchangeName::BinanceSpot => "binance_spot",
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
            "binance_spot" => Some(ExchangeName::BinanceSpot),
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
///
/// `symbol` is always the **venue asset id** (Polymarket: per-window
/// clobTokenId; Binance: e.g. `btcusdt`). For rolling Polymarket /
/// Kalshi topics the per-window forward hook also stamps
/// `full_slug` (e.g. `btc-updown-15m-1715423400`) so subscribers on
/// the stable rolling topic `polymarket:{base_slug}` can detect
/// rollovers without parsing the topic string. Static exchanges leave
/// `full_slug` as `None`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderbookSnapshot {
    pub exchange: ExchangeName,
    pub symbol: String,
    /// Window identifier for rolling markets. `None` for static
    /// exchanges and for older encoded snapshots (back-compat).
    #[serde(default)]
    pub full_slug: Option<String>,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    /// Exchange event time in ns since UNIX epoch (Binance `E`, Polymarket
    /// `last_update`), as reported by the venue. `0` if the venue didn't
    /// supply one. Cross-machine clock — compare only as a rough wire-time
    /// indicator (subject to clock skew; run NTP/chrony). `serde(default)`
    /// for back-compat with older encoded snapshots.
    #[serde(default)]
    pub t_exch_ns: u64,
    /// Wall-clock ns when the orderbook_server read this update off the
    /// exchange WebSocket. Same machine as `timestamp` (the book-applied
    /// time), so `timestamp - t_recv` is trustworthy internal compute time.
    #[serde(default)]
    pub t_recv_ns: u64,
    pub best_bid: Option<PriceLevelTuple>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

impl From<&OrderbookSnapshot> for BbaPayload {
    fn from(snap: &OrderbookSnapshot) -> Self {
        Self {
            exchange: snap.exchange.to_str().to_owned(),
            symbol: snap.symbol.clone(),
            full_slug: snap.full_slug.clone(),
            sequence: snap.sequence,
            timestamp: snap.timestamp,
            t_exch_ns: snap.t_exch_ns,
            t_recv_ns: snap.t_recv_ns,
            best_bid: snap.best_bid,
            best_ask: snap.best_ask,
            spread: snap.spread,
        }
    }
}

/// A single matched trade on the public market channel.
///
/// Distinct from `UserEvent::Fill` (which is a private account fill with
/// `client_oid` / `fee` fields). `LastTradePrice` is broadcast to all
/// subscribers on `MARKET_DATA_SOCKET` with topic `"{exchange}:{symbol}:last_trade"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastTradePrice {
    pub exchange: ExchangeName,
    /// Venue asset id (Polymarket: the YES/NO token id).
    /// See [`OrderbookSnapshot::symbol`] for the symbol/full_slug split.
    pub symbol: String,
    /// Window identifier for rolling markets; `None` for static.
    /// See [`OrderbookSnapshot::full_slug`].
    #[serde(default)]
    pub full_slug: Option<String>,
    pub price: f64,
    pub size: f64,
    /// Taker side: `"BUY"` or `"SELL"`.
    pub side: String,
    pub fee_rate_bps: f64,
    /// Condition market hash (Polymarket: `market` field).
    pub market: String,
    pub timestamp: DateTime<Utc>,
    /// Exchange-assigned trade identifier. Present when the upstream feed
    /// emits one (e.g. Binance `t`); `None` for sources that don't (e.g.
    /// Polymarket `last_trade_price`).
    #[serde(default)]
    pub trade_id: Option<i64>,
}
