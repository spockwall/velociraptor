use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceStreamMessage {
    pub stream: String,
    pub data: BinanceDepthData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceDepthData {
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

/// Subscription confirmation: {"result":null,"id":1}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BinanceSubscribeResponse {
    pub result: Option<serde_json::Value>,
    pub id: Option<u64>,
}
