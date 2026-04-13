//! Orderbook server binary.
//!
//! Streams live orderbook data from exchanges over ZMQ.
//!
//! # Usage
//!
//! ```bash
//! # Start with a TOML config file (recommended)
//! cargo run --bin orderbook_server -- --config configs/server.toml
//!
//! # CLI flags only (env vars also supported)
//! cargo run --bin orderbook_server -- --binance btcusdt,ethusdt --depth 10
//! ```

use clap::Parser;
use libs::protocol::ExchangeName;
use orderbook::configs::{Args, TomlConfig, init_logging};
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::publisher::ZmqPublisher;
use orderbook::{OrderbookSystem, OrderbookSystemConfig};
use recorder::{RotationPolicy, StorageConfig, StorageWriter};
use tracing::{error, info};

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    let toml_cfg = match &args.config {
        Some(path) => TomlConfig::load(path),
        None => TomlConfig::default(),
    };

    let pub_endpoint = args
        .pub_endpoint
        .unwrap_or_else(|| toml_cfg.server.pub_endpoint.clone());
    let router_endpoint = args
        .router_endpoint
        .unwrap_or_else(|| toml_cfg.server.router_endpoint.clone());
    let depth = args.depth.unwrap_or(toml_cfg.server.depth);
    let log_level = args
        .log_level
        .unwrap_or_else(|| toml_cfg.logging.level.clone());
    let log_json = args.log_json.unwrap_or(toml_cfg.logging.json);

    init_logging(&log_level, log_json);

    if let Err(e) = run(
        args.binance,
        args.okx,
        args.hyperliquid,
        toml_cfg,
        pub_endpoint,
        router_endpoint,
        depth,
    )
    .await
    {
        error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

// ── Run ───────────────────────────────────────────────────────────────────────

async fn run(
    cli_binance: Option<Vec<String>>,
    cli_okx: Option<Vec<String>>,
    cli_hyperliquid: Option<Vec<String>>,
    toml_cfg: TomlConfig,
    pub_endpoint: String,
    router_endpoint: String,
    depth: usize,
) -> anyhow::Result<()> {
    // CLI flags take precedence over config file values.
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
    let hyperliquid_coins: Vec<String> = cli_hyperliquid.unwrap_or_else(|| {
        if toml_cfg.hyperliquid.enabled {
            toml_cfg.hyperliquid.coins.clone()
        } else {
            vec![]
        }
    });

    if binance_symbols.is_empty() && okx_symbols.is_empty() && hyperliquid_coins.is_empty() {
        anyhow::bail!(
            "No symbols configured. Provide a --config file or use --binance / --okx / --hyperliquid flags."
        );
    }

    info!(
        binance = ?binance_symbols, okx = ?okx_symbols, hyperliquid = ?hyperliquid_coins,
        pub_endpoint = %pub_endpoint, router_endpoint = %router_endpoint, depth,
        "Starting orderbook server"
    );

    // ── Exchange connections ───────────────────────────────────────────────────

    let mut system_config = OrderbookSystemConfig::new();

    if !binance_symbols.is_empty() {
        let refs: Vec<&str> = binance_symbols.iter().map(String::as_str).collect();
        system_config.with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
                BinanceSubMsgBuilder::new()
                    .with_orderbook_channel(&refs)
                    .build(),
            ),
        );
    }

    if !okx_symbols.is_empty() {
        let refs: Vec<&str> = okx_symbols.iter().map(String::as_str).collect();
        system_config.with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
                OkxSubMsgBuilder::new()
                    .with_orderbook_channel_multi(refs, "SPOT")
                    .build(),
            ),
        );
    }

    if !hyperliquid_coins.is_empty() {
        let refs: Vec<&str> = hyperliquid_coins.iter().map(String::as_str).collect();
        system_config.with_exchange(
            ConnectionConfig::new(ExchangeName::Hyperliquid).set_subscription_message(
                HyperliquidSubMsgBuilder::new().with_coins(&refs).build(),
            ),
        );
    }

    system_config.validate()?;

    // ── System bootstrap ──────────────────────────────────────────────────────

    let system_control = SystemControl::new();
    let mut system = OrderbookSystem::new(system_config, system_control.clone())?;

    system.attach_zmq_publisher(ZmqPublisher::new(
        &pub_endpoint,
        &router_endpoint,
        depth,
        system.channel_request_sender(),
    ));

    if toml_cfg.storage.enabled {
        let rotation = match toml_cfg.storage.rotation.as_str() {
            "none" => RotationPolicy::None,
            _ => RotationPolicy::Daily,
        };
        let zstd_level = match toml_cfg.storage.zstd_level {
            0 => None,
            l => Some(l as i32),
        };
        let storage_config = StorageConfig {
            base_path: toml_cfg.storage.base_path.into(),
            depth: toml_cfg.storage.depth,
            flush_interval_ms: toml_cfg.storage.flush_interval,
            rotation,
            zstd_level,
        };
        info!(
            base_path = %storage_config.base_path.display(),
            depth = storage_config.depth,
            zstd_level = ?storage_config.zstd_level,
            "Storage recorder enabled"
        );
        let rx = system.engine().subscribe_as_recorder(storage_config.depth);
        system.attach_handle(StorageWriter::new(storage_config).start(rx));
    }

    info!("ZMQ PUB    {pub_endpoint}");
    info!("ZMQ ROUTER {router_endpoint}");
    info!("Ready — waiting for subscribers");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C — shutting down");
            system_control.shutdown();
        }
        result = system.run() => { result?; }
    }

    Ok(())
}
