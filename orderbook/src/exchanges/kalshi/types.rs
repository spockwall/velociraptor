use crate::connection::{BaseClientMessage, BasicClientMsgTrait};
use serde::{Deserialize, Serialize};

/// A single `[price_dollars, contract_count_fp]` level from Kalshi's orderbook.
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
    /// Signed contract-count change as a decimal string (e.g. `"-54.00"`).
    pub delta_fp: String,
    /// `"yes"` (bid side) or `"no"` (ask side).
    pub side: String,
    /// Deprecated RFC3339 timestamp string. Optional.
    #[serde(default)]
    pub ts: Option<String>,
    /// Preferred Unix timestamp in milliseconds. Optional.
    #[serde(default)]
    pub ts_ms: Option<i64>,
}

/// Payload of a `type: "trade"` public-trades message.
///
/// Decimal strings are parsed only when converting into the common public
/// trade event, keeping wire deserialization lossless.
#[derive(Debug, Deserialize)]
pub struct KalshiTradeMsg {
    pub trade_id: String,
    pub market_ticker: String,
    pub yes_price_dollars: String,
    pub no_price_dollars: String,
    pub count_fp: String,
    pub taker_side: String,
    /// Added to newer payloads; older documented examples omit it.
    #[serde(default)]
    pub taker_outcome_side: Option<String>,
    /// Added to newer payloads; older documented examples omit it.
    #[serde(default)]
    pub taker_book_side: Option<String>,
    /// Deprecated Unix timestamp in seconds.
    #[serde(default)]
    pub ts: Option<i64>,
    /// Exchange-stamped trade timestamp in Unix milliseconds.
    pub ts_ms: i64,
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

/// A windowed average attached to a CF Benchmarks value update.
///
/// Values remain decimal strings so consumers do not lose precision before
/// choosing their own numeric representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KalshiCfBenchmarksAverage {
    pub value: String,
    pub window_size: u64,
    pub window_start_ts_ms: i64,
    pub window_end_ts_exclusive: i64,
}

/// Parsed form of the JSON string carried in the wire-level `data` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KalshiCfBenchmarksSourceValue {
    #[serde(rename = "type")]
    pub value_type: String,
    pub id: String,
    /// Upstream source timestamp in Unix milliseconds.
    pub time: i64,
    pub value: String,
}

/// A complete typed `cfbenchmarks_value` update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KalshiCfBenchmarksValue {
    pub sid: u64,
    pub seq: u64,
    pub index_id: String,
    /// Time Kalshi received the source frame, in Unix milliseconds.
    pub received_at: i64,
    /// Original JSON string supplied by Kalshi in the `data` field.
    pub raw_data: String,
    /// Parsed representation of [`Self::raw_data`].
    pub source_data: KalshiCfBenchmarksSourceValue,
    pub avg_60s_data: KalshiCfBenchmarksAverage,
    /// Present only during the final minute before a quarter-hour close.
    pub last_60s_windowed_average_15min: Option<KalshiCfBenchmarksAverage>,
    /// Local receive timestamp in Unix nanoseconds.
    pub recv_timestamp: i64,
}

/// Messages emitted by [`crate::exchanges::kalshi::KalshiCfBenchmarksClient`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KalshiCfBenchmarksMessage {
    Value(Box<KalshiCfBenchmarksValue>),
    Base(BaseClientMessage),
}

impl BasicClientMsgTrait for KalshiCfBenchmarksMessage {
    fn connected() -> Self {
        Self::Base(BaseClientMessage::Connected)
    }

    fn disconnected() -> Self {
        Self::Base(BaseClientMessage::Disconnected)
    }

    fn ping() -> Self {
        Self::Base(BaseClientMessage::Ping)
    }

    fn pong() -> Self {
        Self::Base(BaseClientMessage::Pong)
    }

    fn error(error: String) -> Self {
        Self::Base(BaseClientMessage::Error(error))
    }
}

/// Wire payload nested under `msg` for a `cfbenchmarks_value` envelope.
#[derive(Debug, Deserialize)]
pub(crate) struct KalshiCfBenchmarksValueMsg {
    pub index_id: String,
    pub received_at: i64,
    pub data: String,
    pub avg_60s_data: KalshiCfBenchmarksAverage,
    #[serde(default)]
    pub last_60s_windowed_average_15min: Option<KalshiCfBenchmarksAverage>,
}
