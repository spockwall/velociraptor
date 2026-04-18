use crate::types::orderbook::OrderbookUpdate;
use libs::protocol::{StreamSnapshot, UserEvent};
use tokio::sync::broadcast;

pub use libs::protocol::PriceLevelTuple;

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
