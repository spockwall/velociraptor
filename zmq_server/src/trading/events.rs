//! Engine→transport event types.
//!
//! These are internal to the `zmq_server` crate: they carry pre-materialized
//! orderbook depth that only makes sense once the engine has applied an update.
//! They are not part of the cross-process wire protocol (see `libs::protocol`
//! for `UserEvent` and the executor/control schemas).

use chrono::{DateTime, Utc};
use libs::protocol::{ExchangeName, UserEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Transport-level orderbook snapshot — what the ZMQ server broadcasts.
///
/// Unlike the in-memory `OrderbookSnapshot` in the `orderbook` crate, this
/// carries pre-serialized depth levels so it can cross module boundaries
/// without pulling in `Orderbook`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderbookSnapshotPayload {
    pub exchange: ExchangeName,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

/// Raw pre-apply update — emitted before a book mutation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderbookRawUpdate {
    pub exchange: ExchangeName,
    pub symbol: String,
    pub timestamp: DateTime<Utc>,
}

/// Unified engine event — market data + user events on a single broadcast.
///
/// Produced by `TradingEngine`, consumed by any `EngineEventSource` subscriber
/// (e.g. `ZmqServer`).
#[derive(Clone, Debug)]
pub enum EngineEvent {
    OrderbookRaw(OrderbookRawUpdate),
    OrderbookSnapshot(OrderbookSnapshotPayload),
    User(UserEvent),
}

/// Source of engine events for downstream consumers.
///
/// Implemented by `TradingEngineBus` so `ZmqServer` can subscribe without
/// holding a direct reference to the engine.
pub trait EngineEventSource: Send + Sync + 'static {
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent>;
}

/// Dynamic request to add a new exchange/symbol channel at runtime.
/// Flows from `ZmqServer` (received on ROUTER) back to the `TradingSystem`
/// that spawns the new connector.
#[derive(Debug, Clone)]
pub struct ChannelRequest {
    pub client_id: Vec<u8>,
    pub exchange: String,
    pub symbol: String,
}
