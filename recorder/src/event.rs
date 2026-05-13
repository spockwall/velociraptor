pub use libs::protocol::LastTradePrice;
pub use libs::protocol::OrderbookSnapshot;
pub use libs::protocol::UserEvent;

/// Recorder's own event type.
///
/// Snapshots and trades are per-(exchange, symbol) → daily file per pair.
/// User events are account-wide → daily file per kind (`fills`, `orders`,
/// `balance`, `position`).
#[derive(Clone, Debug)]
pub enum RecorderEvent {
    Snapshot(OrderbookSnapshot),
    Trade(LastTradePrice),
    UserEvent(UserEvent),
    /// Non-snapshot events are carried as a unit — writer ignores them.
    RawUpdate,
}
