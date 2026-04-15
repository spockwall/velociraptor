use serde::Deserialize;

/// A single `[price_dollars, size_dollars]` level from Kalshi's orderbook.
/// Both are decimal strings (e.g. `["0.0800", "300.00"]`).
pub type KalshiLevel = [String; 2];

/// Payload of a `type: "orderbook_snapshot"` message.
///
/// `yes_dollars_fp` → YES-side levels, treated as bids.
/// `no_dollars_fp`  → NO-side levels, treated as asks.
#[derive(Debug, Deserialize)]
pub struct KalshiSnapshotMsg {
    pub market_ticker: String,
    #[serde(default)]
    pub market_id: Option<String>,
    #[serde(default)]
    pub yes_dollars_fp: Vec<KalshiLevel>,
    #[serde(default)]
    pub no_dollars_fp: Vec<KalshiLevel>,
}

/// Payload of a `type: "orderbook_delta"` message.
///
/// `delta_fp` may be negative (level shrank or was removed at that price).
#[derive(Debug, Deserialize)]
pub struct KalshiDeltaMsg {
    pub market_ticker: String,
    #[serde(default)]
    pub market_id: Option<String>,
    /// Price in dollars as a decimal string (e.g. `"0.960"`).
    pub price_dollars: String,
    /// Signed size change in dollars as a decimal string (e.g. `"-54.00"`).
    pub delta_fp: String,
    /// `"yes"` (bid side) or `"no"` (ask side).
    pub side: String,
    /// RFC3339 timestamp string. Optional.
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
