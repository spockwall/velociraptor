use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BinanceConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BinanceSpotConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}
