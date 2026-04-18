pub use libs::protocol::OrderbookSnapshot;

/// Recorder's own event type.
#[derive(Clone, Debug)]
pub enum RecorderEvent {
    Snapshot(OrderbookSnapshot),
    /// Non-snapshot events are carried as a unit — writer ignores them.
    RawUpdate,
}
