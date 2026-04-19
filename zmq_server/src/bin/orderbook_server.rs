//! Orderbook server binary.
//!
//! Reads all configuration from a YAML file and starts the ZMQ server.
//! Exchange wiring is delegated to `zmq_server::setup`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin orderbook_server -- --config configs/server.yaml
//! ```

use anyhow::Result;
use clap::Parser;
use libs::configs::Config;
use orderbook::configs::init_logging;
use orderbook::connection::SystemControl;
use orderbook::StreamEngine;
use tracing::error;
use zmq_server::setup::{
    add_binance, add_hyperliquid, add_okx, attach_storage, attach_storage_writer, attach_zmq,
    build_system, spawn_kalshi_schedulers, spawn_polymarket_schedulers,
};
use zmq_server::StreamSystemConfig;

const KALSHI_CREDENTIALS_PATH: &str = "credentials/kalshi.yaml";

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_server",
    about = "Real-time orderbook server — streams live data from exchanges over ZMQ"
)]
struct Args {
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/server.yaml")]
    config: String,

    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    #[arg(long, env = "LOG_JSON", default_value_t = false)]
    log_json: bool,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    init_logging(&args.log_level, args.log_json);

    if let Err(e) = run(&args.config).await {
        error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(config_path: &str) -> Result<()> {
    let cfg = Config::load(config_path);

    // ── Static exchange config ─────────────────────────────────────────────────

    let mut system_cfg = StreamSystemConfig::new();
    system_cfg.set_snapshot_depth(cfg.storage.depth);

    let has_static = [
        cfg.binance.enabled && add_binance(&mut system_cfg, &cfg.binance.symbols),
        cfg.okx.enabled     && add_okx(&mut system_cfg, &cfg.okx.symbols),
        cfg.hyperliquid.enabled && add_hyperliquid(&mut system_cfg, &cfg.hyperliquid.coins),
    ]
    .iter()
    .any(|&v| v);

    let has_polymarket = cfg.polymarket.markets.iter().any(|m| m.enabled);
    let has_kalshi     = cfg.kalshi.market.iter().any(|m| m.enable);

    if !has_static && !has_polymarket && !has_kalshi {
        anyhow::bail!("No exchanges enabled in {config_path}");
    }

    // ── Engine + optional storage hooks ───────────────────────────────────────

    let mut engine = StreamEngine::new(1024, cfg.storage.depth);
    let storage_setup = attach_storage(&mut engine, &cfg);

    // ── Build StreamSystem ────────────────────────────────────────────────────

    let ctrl = SystemControl::new();
    let mut system = build_system(engine, system_cfg, ctrl.clone(), has_static)?;

    // ── ZMQ server + storage writer ───────────────────────────────────────────

    attach_zmq(&mut system, &cfg.server.pub_endpoint, &cfg.server.router_endpoint);
    attach_storage_writer(&mut system, storage_setup);

    // ── Rolling-window schedulers (Polymarket + Kalshi) ───────────────────────

    let _pm  = spawn_polymarket_schedulers(&cfg.polymarket.markets, cfg.storage.depth);
    let _kal = spawn_kalshi_schedulers(&cfg.kalshi.market, cfg.storage.depth, KALSHI_CREDENTIALS_PATH);

    // ── Run until Ctrl-C or engine error ─────────────────────────────────────

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl-C received, shutting down");
            ctrl.shutdown();
        }
        result = system.run() => { result?; }
    }

    Ok(())
}
