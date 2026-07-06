use super::{load_yaml_or_exit, LoggingConfig, ServerConfig, StorageConfig};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KalshiMarketConfig {
    pub enable: bool,
    pub series: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KalshiConfig {
    /// Listed under `market` in the yaml (array of market entries).
    pub market: Vec<KalshiMarketConfig>,
}

/// Top-level shape of `configs/dev/kalshi.yaml` / `configs/prod/kalshi.yaml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KalshiFileConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    pub kalshi: KalshiConfig,
}

impl KalshiFileConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        load_yaml_or_exit(path)
    }

    /// Active (enabled) markets only.
    pub fn active_markets(&self) -> Vec<&KalshiMarketConfig> {
        self.kalshi.market.iter().filter(|m| m.enable).collect()
    }
}
