use crate::orderbook::Orderbook;
use crate::types::orderbook::OrderbookUpdate;
use chrono::{DateTime, Utc};
use libs::protocol::ExchangeName;
use std::sync::Arc;

/// A (price, quantity) pair.
pub type PriceLevelTuple = (f64, f64);

/// Full book state after an update has been applied.
/// Carries an Arc<Orderbook> so consumers can call any Orderbook method
/// (best_bid, spread, vamp, depth, etc.) without data duplication.
#[derive(Clone, Debug)]
pub struct OrderbookSnapshot {
    pub exchange: ExchangeName,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub book: Arc<Orderbook>,
}

/// Events emitted on the broadcast channel after each processed message.
#[derive(Clone, Debug)]
pub enum OrderbookEvent {
    /// Raw wire update emitted BEFORE it is applied to the book.
    RawUpdate(OrderbookUpdate),
    /// Full book state emitted AFTER the update is applied.
    Snapshot(OrderbookSnapshot),
}
