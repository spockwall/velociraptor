use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PolymarketMarketConfig {
    pub enabled: bool,
    pub slug: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PolymarketConfig {
    pub markets: Vec<PolymarketMarketConfig>,
}
