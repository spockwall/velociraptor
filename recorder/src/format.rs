use crate::event::{LastTradePrice, OrderbookSnapshot};
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

/// One persisted last-trade-price record.
///
/// Serialised as MessagePack, prefixed with a `u32 LE` byte length.
/// Written to `{base_path}/{exchange}/{symbol}_trades/{date}.mpack`.
#[derive(Serialize)]
pub struct TradeRecord {
    /// Unix nanosecond timestamp.
    pub ts_ns: i64,
    pub price: f64,
    pub size: f64,
    /// Taker side: `"BUY"` or `"SELL"`.
    pub side: String,
    pub fee_rate_bps: f64,
}

impl TradeRecord {
    pub fn from_trade(trade: &LastTradePrice) -> Self {
        Self {
            ts_ns: trade.timestamp.timestamp_nanos_opt().unwrap_or(0),
            price: trade.price,
            size: trade.size,
            side: trade.side.clone(),
            fee_rate_bps: trade.fee_rate_bps,
        }
    }
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
