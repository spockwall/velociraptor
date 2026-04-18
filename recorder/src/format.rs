use crate::event::OrderbookSnapshot;
use serde::Serialize;

/// One persisted snapshot record.
///
/// Serialised as MessagePack, prefixed with a `u32 LE` byte length.
/// exchange, symbol, spread, mid, and wmid are omitted — they are either
/// encoded in the file path or derivable from bids/asks.
#[derive(Serialize)]
pub struct StorageRecord {
    pub sequence: u64,
    /// Unix nanosecond timestamp — use `pd.to_datetime(ts_ns, unit='ns', utc=True)` in Python.
    pub ts_ns: i64,
    /// Top-N bids as `[price, qty]`, best first.
    pub bids: Vec<[f64; 2]>,
    /// Top-N asks as `[price, qty]`, best first.
    pub asks: Vec<[f64; 2]>,
}

impl StorageRecord {
    pub fn from_snapshot(snap: &OrderbookSnapshot, depth: usize) -> Self {
        Self {
            sequence: snap.sequence,
            ts_ns: snap.timestamp.timestamp_nanos_opt().unwrap_or(0),
            bids: snap.bids[..snap.bids.len().min(depth)]
                .iter()
                .map(|&(p, q)| [p, q])
                .collect(),
            asks: snap.asks[..snap.asks.len().min(depth)]
                .iter()
                .map(|&(p, q)| [p, q])
                .collect(),
        }
    }
}
