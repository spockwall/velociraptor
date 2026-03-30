use serde::Deserialize;

/// A single price level from Hyperliquid l2Book.
/// `px` = price (string), `sz` = size (string), `n` = number of orders.
#[derive(Clone, Debug, Deserialize)]
pub struct HlLevel {
    pub px: String,
    pub sz: String,
    pub n: u64,
}

/// The inner `data` object inside a Hyperliquid l2Book channel message.
/// `levels[0]` = bids (best first), `levels[1]` = asks (best first).
/// `time` is a Unix millisecond timestamp as a JSON number.
#[derive(Clone, Debug, Deserialize)]
pub struct HlBookData {
    pub coin: String,
    pub levels: [Vec<HlLevel>; 2],
    pub time: u64,
}

/// Top-level WebSocket envelope from Hyperliquid.
/// `data` is `None` for control messages like `{"channel":"pong"}`.
#[derive(Clone, Debug, Deserialize)]
pub struct HlWsMessage {
    pub channel: String,
    pub data: Option<serde_json::Value>,
}
