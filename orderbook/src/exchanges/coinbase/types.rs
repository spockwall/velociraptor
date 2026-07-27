use serde::Deserialize;

/// Full Level 2 snapshot sent immediately after subscribing.
#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseSnapshot {
    pub product_id: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

/// Batched Level 2 price-level replacements.
#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseLevel2Update {
    pub product_id: String,
    #[serde(default)]
    pub time: Option<String>,
    pub changes: Vec<[String; 3]>,
}

/// Public execution from the `matches` channel.
///
/// Coinbase's `side` is the maker side, so the parser inverts it to produce
/// the taker direction used by `LastTradePrice`.
#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseMatch {
    pub trade_id: i64,
    pub product_id: String,
    pub time: String,
    pub size: String,
    pub price: String,
    pub side: String,
}
