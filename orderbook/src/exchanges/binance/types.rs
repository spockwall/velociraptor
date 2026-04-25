use serde::{Deserialize, Serialize};

/// Partial book depth message — futures uses short field names with `s`,
/// spot uses long field names and omits the symbol entirely.
///
/// Futures (`fstream.binance.com/ws/<symbol>@depth20@100ms`):
/// `{"s":"BTCUSDT","b":[...],"a":[...]}`
///
/// Spot (`stream.binance.com/ws/<symbol>@depth20@100ms`):
/// `{"lastUpdateId":...,"bids":[...],"asks":[...]}` — no symbol field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceDepthData {
    #[serde(rename = "s", alias = "symbol", default)]
    pub symbol: String,
    #[serde(rename = "b", alias = "bids")]
    pub bids: Vec<[String; 2]>,
    #[serde(rename = "a", alias = "asks")]
    pub asks: Vec<[String; 2]>,
}
/// Subscription confirmation: {"result":null,"id":1}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceSubscribeResponse {
    pub result: Option<serde_json::Value>,
    pub id: Option<u64>,
}

/// Raw trade event from spot `<symbol>@trade` stream.
/// Reference: https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams#trade-streams
#[derive(Clone, Debug, Deserialize)]
pub struct BinanceTradeEvent {
    #[serde(rename = "e")]
    pub event_type: String, // "trade"
    #[serde(rename = "E")]
    pub event_time: i64, // ms
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "t")]
    pub trade_id: i64,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "q")]
    pub qty: String,
    #[serde(rename = "T")]
    pub trade_time: i64, // ms
    #[serde(rename = "m")]
    pub buyer_is_maker: bool,
}
