pub use libs::protocol::StreamSnapshot;

/// Recorder's own event type.
#[derive(Clone, Debug)]
pub enum RecorderEvent {
    Snapshot(StreamSnapshot),
    /// Non-snapshot events are carried as a unit — writer ignores them.
    RawUpdate,
}
