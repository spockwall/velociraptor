pub mod binance;
pub mod hyperliquid;
pub mod kalshi;
pub mod okx;
pub mod polymarket;
pub mod redis;
pub mod server;
pub mod storage;

pub use binance::BinanceConfig;
pub use hyperliquid::HyperliquidConfig;
pub use kalshi::{KalshiConfig, KalshiMarketConfig};
pub use okx::OkxConfig;
pub use polymarket::{PolymarketConfig, PolymarketFileConfig, PolymarketMarketConfig};
pub use redis::RedisConfig;
pub use server::ServerConfig;
pub use storage::StorageConfig;

use serde::Deserialize;
use std::path::Path;

/// Top-level config struct — mirrors the full `example.yaml` layout.
/// Every section is optional; missing sections get their `Default`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub redis: RedisConfig,
    pub binance: BinanceConfig,
    pub okx: OkxConfig,
    pub hyperliquid: HyperliquidConfig,
    pub kalshi: KalshiConfig,
    pub polymarket: PolymarketConfig,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        load_yaml_or_exit(path)
    }
}

/// Read and deserialize a YAML config file into `T`.
/// Returns an error on read or parse failure.
pub fn load_yaml<T, P>(path: P) -> anyhow::Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {e}", path.display()))?;
    serde_yaml::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse '{}': {e}", path.display()))
}

/// Same as [`load_yaml`] but exits the process with a clear message on any error.
pub fn load_yaml_or_exit<T, P>(path: P) -> T
where
    T: for<'de> serde::Deserialize<'de>,
    P: AsRef<Path>,
{
    match load_yaml(path.as_ref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_file(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(rel)
    }

    #[test]
    fn load_example_yaml() {
        let cfg: Config = load_yaml(workspace_file("configs/example.yaml"))
            .expect("configs/example.yaml should parse");

        assert_eq!(cfg.server.pub_endpoint, "tcp://*:5555");
        assert_eq!(cfg.server.router_endpoint, "tcp://*:5556");
        assert_eq!(cfg.server.render_interval, 300);

        assert!(cfg.binance.enabled);
        assert_eq!(cfg.binance.symbols, vec!["btcusdt", "ethusdt"]);

        assert!(cfg.okx.enabled);
        assert_eq!(cfg.okx.symbols, vec!["BTC-USDT", "ETH-USDT-SWAP"]);

        assert!(cfg.hyperliquid.enabled);
        assert_eq!(cfg.hyperliquid.coins, vec!["BTC", "ETH"]);

        assert_eq!(cfg.kalshi.market.len(), 2);
        assert_eq!(cfg.kalshi.market[0].series, "KXBTC15M");
        assert_eq!(cfg.kalshi.market[1].series, "KXETH15M");

        assert_eq!(cfg.polymarket.markets.len(), 4);
        assert!(cfg
            .polymarket
            .markets
            .iter()
            .all(|m| m.interval_secs == 900 || m.interval_secs == 300));
    }
}
