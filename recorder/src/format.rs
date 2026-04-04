use crate::event::RecorderSnapshot;
use serde::Serialize;

/// One persisted snapshot record.
///
/// Serialised as MessagePack, prefixed with a `u32 LE` byte length.
#[derive(Serialize)]
pub struct StorageRecord {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    /// Unix nanosecond timestamp — use `pd.to_datetime(ts_ns, unit='ns', utc=True)` in Python.
    pub ts_ns: i64,
    //pub best_bid_px: Option<f64>,
    //pub best_bid_qty: Option<f64>,
    //pub best_ask_px: Option<f64>,
    //pub best_ask_qty: Option<f64>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    /// Top-N bids as `[price, qty]`, best first.
    pub bids: Vec<[f64; 2]>,
    /// Top-N asks as `[price, qty]`, best first.
    pub asks: Vec<[f64; 2]>,
}

impl StorageRecord {
    pub fn from_snapshot(snap: &RecorderSnapshot, depth: usize) -> Self {
        Self {
            exchange: snap.exchange.clone(),
            symbol: snap.symbol.clone(),
            sequence: snap.sequence,
            ts_ns: snap.timestamp.timestamp_nanos_opt().unwrap_or(0),
            //best_bid_px: snap.best_bid.map(|(p, _)| p),
            //best_bid_qty: snap.best_bid.map(|(_, q)| q),
            //best_ask_px: snap.best_ask.map(|(p, _)| p),
            //best_ask_qty: snap.best_ask.map(|(_, q)| q),
            spread: snap.spread,
            mid: snap.mid,
            wmid: snap.wmid,
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
