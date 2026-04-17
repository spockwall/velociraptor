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
use libs::configs::Config;
use libs::constants::WS_STATUS_SOCKET;
use libs::protocol::ExchangeName;
use orderbook::configs::{Args, init_logging};
use orderbook::connection::{ConnectionConfig, SystemControl};

const DEFAULT_DEPTH: usize = 20;
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use recorder::{RotationPolicy, StorageConfig, StorageWriter};
use std::sync::Arc;
use tracing::{error, info};
use zmq_server::{TradingSystem, TradingSystemConfig, ZmqServer};

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    let cfg = match &args.config {
        Some(path) => Config::load(path),
        None => Config::default(),
    };

    let pub_endpoint = args
        .pub_endpoint
        .unwrap_or_else(|| cfg.server.pub_endpoint.clone());
    let router_endpoint = args
        .router_endpoint
        .unwrap_or_else(|| cfg.server.router_endpoint.clone());
    let depth = args.depth.unwrap_or(DEFAULT_DEPTH);
    let log_level = args.log_level.unwrap_or_else(|| "info".to_string());
    let log_json = args.log_json.unwrap_or(false);

    init_logging(&log_level, log_json);

    if let Err(e) = run(
        args.binance,
        args.okx,
        args.hyperliquid,
        cfg,
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
    cfg: Config,
    pub_endpoint: String,
    router_endpoint: String,
    depth: usize,
) -> anyhow::Result<()> {
    // CLI flags take precedence over config file values.
    let binance_symbols: Vec<String> = cli_binance.unwrap_or_else(|| {
        if cfg.binance.enabled {
            cfg.binance.symbols.clone()
        } else {
            vec![]
        }
    });
    let okx_symbols: Vec<String> = cli_okx.unwrap_or_else(|| {
        if cfg.okx.enabled {
            cfg.okx.symbols.clone()
        } else {
            vec![]
        }
    });
    let hyperliquid_coins: Vec<String> = cli_hyperliquid.unwrap_or_else(|| {
        if cfg.hyperliquid.enabled {
            cfg.hyperliquid.coins.clone()
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

    let mut system_config = TradingSystemConfig::new();
    system_config.set_snapshot_depth(depth);

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
    let mut system = TradingSystem::new(system_config, system_control.clone())?;

    let zmq_handle = ZmqServer::new(
        &pub_endpoint,
        &router_endpoint,
        WS_STATUS_SOCKET,
        system.channel_request_sender(),
    )
    .start(Arc::new(system.engine_bus()));
    system.attach_handle(zmq_handle);

    if cfg.storage.enabled {
        let rotation = match cfg.storage.rotation.as_str() {
            "none" => RotationPolicy::None,
            _ => RotationPolicy::Daily,
        };
        let zstd_level = match cfg.storage.zstd_level {
            0 => None,
            l => Some(l as i32),
        };
        let storage_config = StorageConfig {
            base_path: cfg.storage.base_path.into(),
            depth: cfg.storage.depth,
            flush_interval_ms: cfg.storage.flush_interval,
            rotation,
            zstd_level,
        };
        info!(
            base_path = %storage_config.base_path.display(),
            depth = storage_config.depth,
            zstd_level = ?storage_config.zstd_level,
            "Storage recorder enabled"
        );
        let rx = system.engine().subscribe_as_recorder();
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
