//! Orderbook server binary.
//!
//! Streams live orderbook data from exchanges over ZMQ.
//!
//! # Sockets
//!
//! | Socket | Default          | Purpose                               |
//! |--------|------------------|---------------------------------------|
//! | PUB    | tcp://*:5555     | Throttled snapshot/BBA data           |
//! | ROUTER | tcp://*:5556     | Subscribe / unsubscribe / add_channel |
//!
//! # Configuration (env vars or CLI flags)
//!
//! | Env var              | Flag              | Default           | Description                         |
//! |----------------------|-------------------|-------------------|-------------------------------------|
//! | PUB_ENDPOINT         | --pub-endpoint    | tcp://*:5555      | ZMQ PUB bind address                |
//! | ROUTER_ENDPOINT      | --router-endpoint | tcp://*:5556      | ZMQ ROUTER bind address             |
//! | DEPTH_LEVELS         | --depth           | 20                | Order book depth published          |
//! | BINANCE_SYMBOLS      | --binance         | btcusdt,ethusdt   | Comma-separated Binance symbols     |
//! | OKX_SYMBOLS          | --okx             | (none)            | Comma-separated OKX symbols (SPOT)  |
//! | POLYMARKET_ASSETS    | --polymarket      | (none)            | Comma-separated Polymarket token IDs|
//! | LOG_LEVEL            | --log-level       | info              | Tracing filter (e.g. debug)         |
//! | LOG_JSON             | --log-json        | false             | Emit JSON logs instead of plain     |
//!
//! # Examples
//!
//! ```bash
//! # Defaults — Binance BTCUSDT + ETHUSDT
//! cargo run --bin orderbook_server --release
//!
//! # Custom symbols and depth
//! BINANCE_SYMBOLS=btcusdt,ethusdt,solusdt DEPTH_LEVELS=10 \
//!     cargo run --bin orderbook_server --release
//!
//! # Or via flags
//! ./orderbook_server --binance btcusdt,solusdt --okx BTC-USDT-SWAP --depth 5
//! ```

use clap::Parser;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::PolymarketSubMsgBuilder;
use orderbook::publisher::ZmqPublisher;
use orderbook::types::ExchangeName;
use orderbook::{OrderbookSystem, OrderbookSystemConfig};
use tracing::{error, info};

// ── CLI / env config ──────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_server",
    about = "Real-time orderbook server — streams live data from exchanges over ZMQ",
    version
)]
struct Args {
    /// ZMQ PUB socket bind address (data stream)
    #[arg(long, env = "PUB_ENDPOINT", default_value = "tcp://*:5555")]
    pub_endpoint: String,

    /// ZMQ ROUTER socket bind address (subscribe / unsubscribe / add_channel)
    #[arg(long, env = "ROUTER_ENDPOINT", default_value = "tcp://*:5556")]
    router_endpoint: String,

    /// Order book depth published per snapshot
    #[arg(long, env = "DEPTH_LEVELS", default_value_t = 20)]
    depth: usize,

    /// Comma-separated Binance symbols to stream (lowercase, e.g. btcusdt,ethusdt)
    #[arg(long, env = "BINANCE_SYMBOLS", value_delimiter = ',')]
    binance: Option<Vec<String>>,

    /// Comma-separated OKX SPOT symbols to stream (e.g. BTC-USDT,ETH-USDT)
    #[arg(long, env = "OKX_SYMBOLS", value_delimiter = ',')]
    okx: Option<Vec<String>>,

    /// Comma-separated Polymarket asset (token) IDs to stream
    #[arg(long, env = "POLYMARKET_ASSETS", value_delimiter = ',')]
    polymarket: Option<Vec<String>>,

    /// Tracing filter string (e.g. info, debug, orderbook=trace)
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Emit JSON-formatted logs (useful for log aggregators)
    #[arg(long, env = "LOG_JSON", default_value_t = false)]
    log_json: bool,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Load .env if present (ignored if not found)
    let _ = dotenvy::dotenv();

    let args = Args::parse();

    init_logging(&args.log_level, args.log_json);

    if let Err(e) = run(args).await {
        error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    let binance_symbols: Vec<String> = args
        .binance
        .unwrap_or_else(|| vec!["btcusdt".into(), "ethusdt".into()]);

    let okx_symbols: Vec<String> = args.okx.unwrap_or_default();
    let polymarket_assets: Vec<String> = args.polymarket.unwrap_or_default();

    if binance_symbols.is_empty() && okx_symbols.is_empty() && polymarket_assets.is_empty() {
        anyhow::bail!("No symbols configured. Use --binance, --okx, or --polymarket.");
    }

    info!(
        binance = ?binance_symbols,
        okx = ?okx_symbols,
        polymarket = ?polymarket_assets,
        pub_endpoint = %args.pub_endpoint,
        router_endpoint = %args.router_endpoint,
        depth = args.depth,
        "Starting orderbook server"
    );

    let mut config = OrderbookSystemConfig::new();

    // ── Binance connector ─────────────────────────────────────────────────────
    if !binance_symbols.is_empty() {
        let sym_refs: Vec<&str> = binance_symbols.iter().map(String::as_str).collect();
        let sub_msg = BinanceSubMsgBuilder::new()
            .with_orderbook_channel(&sym_refs)
            .build();
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(sub_msg),
        );
    }

    // ── OKX connector ────────────────────────────────────────────────────────
    if !okx_symbols.is_empty() {
        let sym_refs: Vec<&str> = okx_symbols.iter().map(String::as_str).collect();
        let sub_msg = OkxSubMsgBuilder::new()
            .with_orderbook_channel_multi(sym_refs, "SPOT")
            .build();
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(sub_msg),
        );
    }

    // ── Polymarket connector ──────────────────────────────────────────────────
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

    config.validate()?;

    let system_control = SystemControl::new();
    let mut system = OrderbookSystem::new(config, system_control.clone())?;

    let channel_tx = system.channel_request_sender();
    system.attach_zmq_publisher(ZmqPublisher::new(
        &args.pub_endpoint,
        &args.router_endpoint,
        args.depth,
        channel_tx,
    ));

    info!("ZMQ PUB    {}", args.pub_endpoint);
    info!("ZMQ ROUTER {}", args.router_endpoint);
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

    let env_filter = EnvFilter::try_new(filter)
        .unwrap_or_else(|_| EnvFilter::new("info"));

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }
}
