use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HyperliquidConfig {
    pub enabled: bool,
    /// Uppercase coin names (e.g. `BTC`, `ETH`).
    pub coins: Vec<String>,
}
