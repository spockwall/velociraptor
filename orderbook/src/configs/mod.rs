pub mod exchanges;
pub mod logging;
pub mod server;
pub mod storage;

pub use exchanges::{
    BinanceConfig, HyperliquidConfig, KalshiConfig, OkxConfig, PolymarketArgs, PolymarketConfig,
    PolymarketMarket, PolymarketTomlConfig,
};
pub use logging::{LoggingConfig, init_logging};
pub use server::{Args, ServerConfig};
pub use storage::StorageConfig;

use serde::Deserialize;

/// Root TOML config — mirrors `configs/server.toml` section by section.
#[derive(Debug, Deserialize, Default)]
pub struct TomlConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub binance: BinanceConfig,
    #[serde(default)]
    pub okx: OkxConfig,
    #[serde(default)]
    pub hyperliquid: HyperliquidConfig,
    #[serde(default)]
    pub kalshi: KalshiConfig,
    #[serde(default)]
    pub polymarket: Vec<PolymarketMarket>,
    #[serde(default)]
    pub storage: StorageConfig,
}

impl TomlConfig {
    /// Load from a TOML file path, exiting the process on any error.
    pub fn load(path: &str) -> Self {
        let contents = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to read config file '{path}': {e}");
                std::process::exit(1);
            }
        };
        match toml::from_str(&contents) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to parse config file '{path}': {e}");
                std::process::exit(1);
            }
        }
    }
}
