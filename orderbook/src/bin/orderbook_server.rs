//! Orderbook server binary.
//!
//! Streams live orderbook data from exchanges over ZMQ.
//!
//! # Usage
//!
//! ```bash
//! # Start with a TOML config file (recommended)
//! cargo run --bin orderbook_server -- --config config/server.toml
//!
//! # CLI flags only (env vars also supported)
//! cargo run --bin orderbook_server -- --binance btcusdt,ethusdt --depth 10
//! ```
//!
//! # Config file takes precedence over CLI defaults.
//! # CLI flags take precedence over config file values.

use clap::Parser;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::PolymarketSubMsgBuilder;
use orderbook::publisher::ZmqPublisher;
use orderbook::types::ExchangeName;
use orderbook::{OrderbookSystem, OrderbookSystemConfig};
use serde::Deserialize;
use tracing::{error, info};

// ── TOML config structs ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct TomlConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    binance: BinanceConfig,
    #[serde(default)]
    okx: OkxConfig,
    #[serde(default)]
    polymarket: Vec<PolymarketMarket>,
    #[serde(default)]
    hyperliquid: HyperliquidConfig,
}

#[derive(Debug, Deserialize, Default)]
struct HyperliquidConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    coins: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    #[serde(default = "default_pub_endpoint")]
    pub_endpoint: String,
    #[serde(default = "default_router_endpoint")]
    router_endpoint: String,
    #[serde(default = "default_depth")]
    depth: usize,
    #[serde(default = "default_log_level")]
    log_level: String,
    #[serde(default)]
    log_json: bool,
}

#[derive(Debug, Deserialize, Default)]
struct BinanceConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OkxConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PolymarketMarket {
    #[serde(default = "bool_true")]
    enabled: bool,
    #[serde(default)]
    question: String,
    yes: String,
    no: String,
}

fn default_pub_endpoint() -> String    { "tcp://*:5555".into() }
fn default_router_endpoint() -> String { "tcp://*:5556".into() }
fn default_depth() -> usize            { 20 }
fn default_log_level() -> String       { "info".into() }
fn bool_true() -> bool                 { true }

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            pub_endpoint:    default_pub_endpoint(),
            router_endpoint: default_router_endpoint(),
            depth:           default_depth(),
            log_level:       default_log_level(),
            log_json:        false,
        }
    }
}

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_server",
    about = "Real-time orderbook server — streams live data from exchanges over ZMQ",
    version
)]
struct Args {
    /// Path to TOML config file
    #[arg(long, env = "CONFIG_FILE")]
    config: Option<String>,

    /// ZMQ PUB socket bind address (overrides config file)
    #[arg(long, env = "PUB_ENDPOINT")]
    pub_endpoint: Option<String>,

    /// ZMQ ROUTER socket bind address (overrides config file)
    #[arg(long, env = "ROUTER_ENDPOINT")]
    router_endpoint: Option<String>,

    /// Order book depth published per snapshot (overrides config file)
    #[arg(long, env = "DEPTH_LEVELS")]
    depth: Option<usize>,

    /// Comma-separated Binance symbols (overrides config file)
    #[arg(long, env = "BINANCE_SYMBOLS", value_delimiter = ',')]
    binance: Option<Vec<String>>,

    /// Comma-separated OKX SPOT symbols (overrides config file)
    #[arg(long, env = "OKX_SYMBOLS", value_delimiter = ',')]
    okx: Option<Vec<String>>,

    /// Comma-separated Hyperliquid coin symbols, e.g. BTC,ETH (overrides config file)
    #[arg(long, env = "HYPERLIQUID_COINS", value_delimiter = ',')]
    hyperliquid: Option<Vec<String>>,

    /// Tracing filter string (overrides config file)
    #[arg(long, env = "LOG_LEVEL")]
    log_level: Option<String>,

    /// Emit JSON-formatted logs (overrides config file)
    #[arg(long, env = "LOG_JSON")]
    log_json: Option<bool>,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    let args = Args::parse();

    // Load TOML config if provided, otherwise use defaults
    let toml_cfg: TomlConfig = match &args.config {
        Some(path) => {
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
        None => TomlConfig::default(),
    };

    // CLI flags override config file
    let pub_endpoint    = args.pub_endpoint.unwrap_or_else(|| toml_cfg.server.pub_endpoint.clone());
    let router_endpoint = args.router_endpoint.unwrap_or_else(|| toml_cfg.server.router_endpoint.clone());
    let depth           = args.depth.unwrap_or(toml_cfg.server.depth);
    let log_level       = args.log_level.unwrap_or_else(|| toml_cfg.server.log_level.clone());
    let log_json        = args.log_json.unwrap_or(toml_cfg.server.log_json);

    init_logging(&log_level, log_json);

    if let Err(e) = run(args.binance, args.okx, args.hyperliquid, toml_cfg, pub_endpoint, router_endpoint, depth).await {
        error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(
    cli_binance: Option<Vec<String>>,
    cli_okx: Option<Vec<String>>,
    cli_hyperliquid: Option<Vec<String>>,
    toml_cfg: TomlConfig,
    pub_endpoint: String,
    router_endpoint: String,
    depth: usize,
) -> anyhow::Result<()> {
    // CLI flags override config file; config file overrides built-in defaults
    let binance_symbols: Vec<String> = cli_binance.unwrap_or_else(|| {
        if toml_cfg.binance.enabled {
            toml_cfg.binance.symbols.clone()
        } else {
            vec![]
        }
    });

    let okx_symbols: Vec<String> = cli_okx.unwrap_or_else(|| {
        if toml_cfg.okx.enabled {
            toml_cfg.okx.symbols.clone()
        } else {
            vec![]
        }
    });

    // Collect enabled Polymarket markets from config
    let polymarket_assets: Vec<String> = toml_cfg
        .polymarket
        .iter()
        .filter(|m| m.enabled)
        .flat_map(|m| [m.yes.clone(), m.no.clone()])
        .collect();

    let hyperliquid_coins: Vec<String> = cli_hyperliquid.unwrap_or_else(|| {
        if toml_cfg.hyperliquid.enabled {
            toml_cfg.hyperliquid.coins.clone()
        } else {
            vec![]
        }
    });

    if binance_symbols.is_empty()
        && okx_symbols.is_empty()
        && polymarket_assets.is_empty()
        && hyperliquid_coins.is_empty()
    {
        anyhow::bail!(
            "No symbols configured. Provide a --config file or use --binance / --okx / --hyperliquid flags."
        );
    }

    info!(
        binance  = ?binance_symbols,
        okx      = ?okx_symbols,
        hyperliquid = ?hyperliquid_coins,
        polymarket_markets = toml_cfg.polymarket.iter().filter(|m| m.enabled).count(),
        pub_endpoint  = %pub_endpoint,
        router_endpoint = %router_endpoint,
        depth,
        "Starting orderbook server"
    );

    // Log Polymarket market names for visibility
    for m in toml_cfg.polymarket.iter().filter(|m| m.enabled) {
        info!(question = %m.question, yes = %m.yes, no = %m.no, "Polymarket market");
    }

    let mut config = OrderbookSystemConfig::new();

    // ── Binance ───────────────────────────────────────────────────────────────
    if !binance_symbols.is_empty() {
        let sym_refs: Vec<&str> = binance_symbols.iter().map(String::as_str).collect();
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
                BinanceSubMsgBuilder::new().with_orderbook_channel(&sym_refs).build(),
            ),
        );
    }

    // ── OKX ───────────────────────────────────────────────────────────────────
    if !okx_symbols.is_empty() {
        let sym_refs: Vec<&str> = okx_symbols.iter().map(String::as_str).collect();
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
                OkxSubMsgBuilder::new().with_orderbook_channel_multi(sym_refs, "SPOT").build(),
            ),
        );
    }

    // ── Polymarket ────────────────────────────────────────────────────────────
    if !polymarket_assets.is_empty() {
        let mut builder = PolymarketSubMsgBuilder::new();
        for asset_id in &polymarket_assets {
            builder = builder.with_asset(asset_id);
        }
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Polymarket)
                .set_subscription_message(builder.build()),
        );
    }

    // ── Hyperliquid ───────────────────────────────────────────────────────────
    if !hyperliquid_coins.is_empty() {
        let coin_refs: Vec<&str> = hyperliquid_coins.iter().map(String::as_str).collect();
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Hyperliquid).set_subscription_message(
                HyperliquidSubMsgBuilder::new().with_coins(&coin_refs).build(),
            ),
        );
    }

    config.validate()?;

    let system_control = SystemControl::new();
    let mut system = OrderbookSystem::new(config, system_control.clone())?;

    let channel_tx = system.channel_request_sender();
    system.attach_zmq_publisher(ZmqPublisher::new(
        &pub_endpoint,
        &router_endpoint,
        depth,
        channel_tx,
    ));

    info!("ZMQ PUB    {pub_endpoint}");
    info!("ZMQ ROUTER {router_endpoint}");
    info!("Ready — waiting for subscribers");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C — shutting down");
            system_control.shutdown();
        }
        result = system.run() => {
            result?;
        }
    }

    Ok(())
}

// ── Logging ───────────────────────────────────────────────────────────────────

fn init_logging(filter: &str, json: bool) {
    use tracing_subscriber::EnvFilter;
    let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        tracing_subscriber::fmt().json().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }
}
