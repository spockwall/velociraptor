use super::{load_yaml_or_exit, ServerConfig, StorageConfig};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PolymarketMarketConfig {
    pub enabled: bool,
    pub slug: String,
    pub interval_secs: u64,
}

/// The `polymarket:` section — holds the list of markets.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PolymarketConfig {
    pub markets: Vec<PolymarketMarketConfig>,
}

/// Top-level shape of `configs/polymarket.yaml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PolymarketFileConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub polymarket: PolymarketConfig,
}

impl PolymarketFileConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        load_yaml_or_exit(path)
    }

    /// Active (enabled) markets only.
    pub fn active_markets(&self) -> Vec<&PolymarketMarketConfig> {
        self.polymarket.markets.iter().filter(|m| m.enabled).collect()
    }
}
