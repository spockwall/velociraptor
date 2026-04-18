use crate::connection::SystemControl;
use crate::orderbook::Orderbook;
use crate::types::orderbook::{OrderbookUpdate, StreamMessage};
use chrono::{DateTime, Utc};
use libs::protocol::{ExchangeName, UserEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

/// A (price, quantity) pair.
pub type PriceLevelTuple = (f64, f64);

/// Materialized orderbook snapshot broadcast to subscribers.
///
/// Carries pre-computed depth, BBA, and derived prices so every consumer
/// (recorder, ZMQ transport, hooks) gets the same serializable shape. Built
/// from an `Orderbook` via `StreamSnapshot::from_book` at emission time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamSnapshot {
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

impl StreamSnapshot {
    pub fn from_book(book: &Orderbook, depth: usize) -> Self {
        let (bids, asks) = book.depth(depth);
        Self {
            exchange: book.exchange.clone(),
            symbol: book.symbol.clone(),
            sequence: book.sequence,
            timestamp: book.last_update,
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

/// Events emitted on the broadcast channel after each processed message.
#[derive(Clone, Debug)]
pub enum StreamEvent {
    /// Raw wire update emitted BEFORE it is applied to the book.
    OrderbookRaw(OrderbookUpdate),
    /// Full materialized snapshot emitted AFTER the update is applied.
    OrderbookSnapshot(StreamSnapshot),
    /// User/private channel event (fills, order updates, positions, balances).
    User(UserEvent),
}

/// Source of engine events for downstream consumers (transport layers,
/// observers). Implemented by engine bus handles so subscribers can decouple
/// from the engine's concrete type.
pub trait StreamEventSource: Send + Sync + 'static {
    fn subscribe(&self) -> broadcast::Receiver<StreamEvent>;
}

/// Dynamic request for a new exchange/symbol channel at runtime.
/// Flows from the transport layer back to the `StreamSystem` (or equivalent)
/// which hands it to its registered `ChannelSpawner`.
#[derive(Debug, Clone)]
pub struct ChannelRequest {
    pub client_id: Vec<u8>,
    pub exchange: String,
    pub symbol: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelSpawnError {
    #[error("unknown exchange '{0}'")]
    UnknownExchange(String),
    #[error("channel spawner not configured")]
    Unsupported,
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
}

/// Extension point for `StreamSystem`: turns a `ChannelRequest` into a running
/// connector task that pushes `StreamMessage`s through `message_tx`.
pub trait ChannelSpawner: Send + Sync + 'static {
    fn spawn(
        &self,
        req: ChannelRequest,
        message_tx: mpsc::UnboundedSender<StreamMessage>,
        ctrl: SystemControl,
    ) -> Result<tokio::task::JoinHandle<()>, ChannelSpawnError>;
}
