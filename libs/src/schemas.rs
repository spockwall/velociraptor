use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderBookDelta {
    pub exchange: String,
    pub symbol: String,
    /// Action type: "partial", "update", "delete", "insert", "unknown"
    pub action: String,
    pub timestamp_ms: i64,
    pub sequence: i64,
    pub prev_sequence: i64,
    /// Payload contains "asks" and "bids" as lists of (price, qty) tuples
    pub payload: HashMap<String, Vec<(f64, f64)>>,
}

// ── Binance WebSocket schemas ─────────────────────────────────────────────────

/// Depth stream event (`<symbol>@depth`)
/// `wss://fstream.binance.com/public/ws/<symbol>@depth`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceDepthUpdate {
    /// Event type, e.g. "depthUpdate"
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time (ms)
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Transaction time (ms)
    #[serde(rename = "T")]
    pub transaction_time: i64,
    /// Symbol, e.g. "BTCUSDT"
    #[serde(rename = "s")]
    pub symbol: String,
    /// First update ID in event
    #[serde(rename = "U")]
    pub first_update_id: i64,
    /// Final update ID in event
    #[serde(rename = "u")]
    pub final_update_id: i64,
    /// Final update ID in last stream (`u` of previous event)
    #[serde(rename = "pu")]
    pub prev_final_update_id: i64,
    /// Bids to be updated: (price, qty) string pairs
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    /// Asks to be updated: (price, qty) string pairs
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

/// REST snapshot result (`method: "depth"`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceDepthResult {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: i64,
    /// Message output time (ms)
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Transaction time (ms)
    #[serde(rename = "T")]
    pub transaction_time: i64,
    /// Bids: (price, qty) string pairs
    pub bids: Vec<[String; 2]>,
    /// Asks: (price, qty) string pairs
    pub asks: Vec<[String; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceRateLimit {
    #[serde(rename = "rateLimitType")]
    pub rate_limit_type: String,
    pub interval: String,
    #[serde(rename = "intervalNum")]
    pub interval_num: u32,
    pub limit: u32,
    pub count: u32,
}

/// Envelope for a REST-over-WebSocket response (`method: "depth"`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceWsResponse {
    pub id: String,
    pub status: u16,
    pub result: BinanceDepthResult,
    #[serde(rename = "rateLimits")]
    pub rate_limits: Vec<BinanceRateLimit>,
}

/// Request sent over the WebSocket connection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceWsRequest {
    pub id: String,
    pub method: String,
    pub params: BinanceWsRequestParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinanceWsRequestParams {
    pub symbol: String,
}
