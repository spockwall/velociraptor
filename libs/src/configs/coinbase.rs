use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CoinbaseConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}
