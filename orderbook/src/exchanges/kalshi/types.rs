use serde::Deserialize;

/// A single [price, size] level from Kalshi's orderbook, represented as
/// string decimals (e.g. ["0.0800", "300.00"]).
pub type KalshiLevel = [String; 2];

/// Payload of a `type: "orderbook_snapshot"` message.
///
/// `yes_dollars_fp` → bid side (YES contracts, price in dollars)
/// `no_dollars_fp`  → ask side (NO contracts, price in dollars)
#[derive(Debug, Deserialize)]
pub struct KalshiSnapshotMsg {
    pub market_ticker: String,
    pub market_id: String,
    /// YES-side levels [price_dollars, size_dollars] — treated as bids.
    #[serde(default)]
    pub yes_dollars_fp: Vec<KalshiLevel>,
    /// NO-side levels [price_dollars, size_dollars] — treated as asks.
    #[serde(default)]
    pub no_dollars_fp: Vec<KalshiLevel>,
}

/// Payload of a `type: "orderbook_delta"` message.
///
/// `delta_fp` may be negative (level shrank or was removed at that price).
#[derive(Debug, Deserialize)]
pub struct KalshiDeltaMsg {
    pub market_ticker: String,
    pub market_id: String,
    pub price_dollars: String,
    /// Signed size change (dollars). Negative means remove/reduce.
    pub delta_fp: String,
    /// "yes" (bid side) or "no" (ask side).
    pub side: String,
    /// RFC3339 timestamp string, e.g. "2022-11-22T20:44:01Z".
    pub ts: Option<String>,
}

/// Top-level WebSocket message envelope from Kalshi.
///
/// All event messages carry `type` + `sid` + `seq` + `msg`.
/// Control messages (subscribe ack, ping) have a different shape and are
/// handled by checking `type` before deserialising `msg`.
#[derive(Debug, Deserialize)]
pub struct KalshiEnvelope {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub sid: Option<u64>,
    pub seq: Option<u64>,
    #[serde(default)]
    pub msg: serde_json::Value,
}
