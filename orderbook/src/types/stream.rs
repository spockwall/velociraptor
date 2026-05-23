use crate::types::orderbook::OrderbookUpdate;
use libs::protocol::{LastTradePrice, OrderbookSnapshot, UserEvent};
use tokio::sync::broadcast;

/// A (price, quantity) pair.
pub type PriceLevelTuple = (f64, f64);
/// Events emitted on the broadcast channel after each processed message.
#[derive(Clone, Debug)]
pub enum StreamEvent {
    /// Raw wire update emitted BEFORE it is applied to the book.
    OrderbookRaw(OrderbookUpdate),
    /// Full materialized snapshot emitted AFTER the update is applied.
    OrderbookSnapshot(OrderbookSnapshot),
    /// User/private channel event (fills, order updates, positions, balances).
    User(UserEvent),
    /// Public market last-trade event — emitted when a maker/taker order is matched.
    LastTradePrice(LastTradePrice),
    /// Rolling-market snapshot tagged with its base_slug, so the ZMQ server
    /// can publish it on the stable topic `{exchange}:{base_slug}` while the
    /// payload's `symbol` (asset_id) and `full_slug` carry the per-window
    /// identity. Static exchanges keep using `OrderbookSnapshot`.
    RollingSnapshot {
        base_slug: String,
        snap: OrderbookSnapshot,
    },
    /// Rolling-market last-trade tagged with base_slug — same rationale as
    /// `RollingSnapshot`. Published on `{exchange}:{base_slug}:last_trade`.
    RollingLastTradePrice {
        base_slug: String,
        trade: LastTradePrice,
    },
}

/// Source of engine events for downstream consumers (transport layers,
/// observers). Implemented by engine bus handles so subscribers can decouple
/// from the engine's concrete type.
pub trait StreamEventSource: Send + Sync + 'static {
    fn subscribe(&self) -> broadcast::Receiver<StreamEvent>;
}
