use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct BinanceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct OkxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct HyperliquidConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub coins: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PolymarketMarket {
    pub enabled: bool,
    pub question: String,
    pub yes: String,
    pub no: String,
}

impl Default for PolymarketMarket {
    fn default() -> Self {
        Self {
            enabled: true,
            question: String::new(),
            yes: String::new(),
            no: String::new(),
        }
    }
}
