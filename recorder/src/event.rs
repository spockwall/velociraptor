use chrono::{DateTime, Utc};

/// A minimal snapshot that recorder needs to write a record.
/// Built from `OrderbookSnapshot` by the caller (server binary).
#[derive(Clone, Debug)]
pub struct RecorderSnapshot {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    /// Top-N bids as (price, qty), best first.
    pub bids: Vec<(f64, f64)>,
    /// Top-N asks as (price, qty), best first.
    pub asks: Vec<(f64, f64)>,
}

/// Recorder's own event type — no dependency on the orderbook crate.
#[derive(Clone, Debug)]
pub enum RecorderEvent {
    Snapshot(RecorderSnapshot),
    /// Non-snapshot events are carried as a unit — writer ignores them.
    RawUpdate,
}
