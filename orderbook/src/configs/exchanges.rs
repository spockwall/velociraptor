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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PolymarketMarket {
    pub enabled: bool,
    /// Base slug, e.g. `"btc-updown-15m"` or a full static slug.
    pub slug: String,
    /// Window size in seconds. 0 = static market (slug used as-is).
    pub interval_secs: u64,
    /// Subscribe to current + next N windows ahead. 0 = current window only.
    pub prefetch_windows: usize,
}

impl Default for PolymarketMarket {
    fn default() -> Self {
        Self {
            enabled: true,
            slug: String::new(),
            interval_secs: 0,
            prefetch_windows: 0,
        }
    }
}
