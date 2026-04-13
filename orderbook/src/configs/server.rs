use clap::Parser;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub pub_endpoint: String,
    pub router_endpoint: String,
    pub depth: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            pub_endpoint: "tcp://*:5555".into(),
            router_endpoint: "tcp://*:5556".into(),
            depth: 20,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_server",
    about = "Real-time orderbook server — streams live data from exchanges over ZMQ",
    version
)]
pub struct Args {
    /// Path to TOML config file
    #[arg(long, env = "CONFIG_FILE")]
    pub config: Option<String>,

    /// ZMQ PUB socket bind address (overrides config file)
    #[arg(long, env = "PUB_ENDPOINT")]
    pub pub_endpoint: Option<String>,

    /// ZMQ ROUTER socket bind address (overrides config file)
    #[arg(long, env = "ROUTER_ENDPOINT")]
    pub router_endpoint: Option<String>,

    /// Order book depth published per snapshot (overrides config file)
    #[arg(long, env = "DEPTH_LEVELS")]
    pub depth: Option<usize>,

    /// Comma-separated Binance symbols (overrides config file)
    #[arg(long, env = "BINANCE_SYMBOLS", value_delimiter = ',')]
    pub binance: Option<Vec<String>>,

    /// Comma-separated OKX SPOT symbols (overrides config file)
    #[arg(long, env = "OKX_SYMBOLS", value_delimiter = ',')]
    pub okx: Option<Vec<String>>,

    /// Comma-separated Hyperliquid coin symbols, e.g. BTC,ETH (overrides config file)
    #[arg(long, env = "HYPERLIQUID_COINS", value_delimiter = ',')]
    pub hyperliquid: Option<Vec<String>>,

    /// Comma-separated Kalshi market tickers, e.g. FED-23DEC-T3.00 (overrides config file)
    #[arg(long, env = "KALSHI_TICKERS", value_delimiter = ',')]
    pub kalshi: Option<Vec<String>>,

    /// Tracing filter string (overrides config file)
    #[arg(long, env = "LOG_LEVEL")]
    pub log_level: Option<String>,

    /// Emit JSON-formatted logs (overrides config file)
    #[arg(long, env = "LOG_JSON")]
    pub log_json: Option<bool>,
}
