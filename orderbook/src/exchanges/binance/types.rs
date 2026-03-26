use serde::{Deserialize, Serialize};

/// Raw partial book depth message from fstream.binance.com/ws/<symbol>@depth20@100ms
/// Fields use short names (b/a) and symbol is in "s".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceDepthData {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

/// Subscription confirmation: {"result":null,"id":1}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceSubscribeResponse {
    pub result: Option<serde_json::Value>,
    pub id: Option<u64>,
}
