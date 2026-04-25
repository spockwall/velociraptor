//! Standalone orderbook recorder.
//!
//! Connects directly to exchanges (Binance, OKX, Hyperliquid, Kalshi,
//! Polymarket) and writes every orderbook snapshot and last-trade event to
//! disk via the `recorder` crate. Runs independently of `orderbook_server` —
//! no ZMQ involved.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin orderbook_recorder --release -- --config configs/server.yaml
//! ```

use anyhow::Result;
use clap::Parser;
use libs::configs::Config;
use libs::protocol::{ExchangeName, LastTradePrice, OrderbookSnapshot};
use orderbook::configs::init_logging;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::orderbook::OrderbookUpdate;
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use recorder::{RecorderEvent, RotationPolicy, StorageConfig, StorageWriter};
use tokio::sync::broadcast;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_recorder",
    about = "Standalone recorder — streams market data from exchanges and writes to disk"
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

    if !cfg.storage.enabled {
        anyhow::bail!("storage.enabled must be true in {config_path}");
    }

    // ── Static exchange subscriptions ─────────────────────────────────────────

    let mut system_cfg = StreamSystemConfig::new();
    system_cfg.set_snapshot_depth(cfg.storage.depth);

    let has_static = [
        cfg.binance.enabled && add_binance(&mut system_cfg, &cfg.binance.symbols),
        cfg.binance_spot.enabled && add_binance_spot(&mut system_cfg, &cfg.binance_spot.symbols),
        cfg.okx.enabled && add_okx(&mut system_cfg, &cfg.okx.symbols),
        cfg.hyperliquid.enabled && add_hyperliquid(&mut system_cfg, &cfg.hyperliquid.coins),
    ]
    .iter()
    .any(|&v| v);

    // ── Engine + storage hooks ────────────────────────────────────────────────

    let storage_config = build_storage_config(&cfg);
    let mut engine = StreamEngine::new(1024, cfg.storage.depth);
    let rec_rx = attach_storage_hooks(&mut engine, &storage_config);

    // ── StreamSystem ──────────────────────────────────────────────────────────

    if !has_static {
        // Need at least one exchange in the config to start the engine.
        system_cfg.with_exchange(
            ClientConfig::new(ExchangeName::Binance).set_subscription_message(String::new()),
        );
    } else {
        system_cfg.validate()?;
    }

    let ctrl = SystemControl::new();
    let mut system = StreamSystem::new(engine, system_cfg, ctrl.clone())?;

    // ── Storage writer task ───────────────────────────────────────────────────

    system.attach_handle(StorageWriter::new(storage_config).start(rec_rx));

    // ── Run until Ctrl-C ──────────────────────────────────────────────────────

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl-C received, shutting down");
            ctrl.shutdown();
        }
        result = system.run() => { result?; }
    }

    Ok(())
}

// ── Storage helpers ───────────────────────────────────────────────────────────

fn build_storage_config(cfg: &Config) -> StorageConfig {
    let rotation = match cfg.storage.rotation.as_str() {
        "none" => RotationPolicy::None,
        _ => RotationPolicy::Daily,
    };
    let zstd_level = match cfg.storage.zstd_level {
        0 => None,
        l => Some(l as i32),
    };
    StorageConfig {
        base_path: cfg.storage.base_path.clone().into(),
        depth: cfg.storage.depth,
        flush_interval_ms: cfg.storage.flush_interval,
        rotation,
        zstd_level,
    }
}

fn attach_storage_hooks(
    engine: &mut StreamEngine,
    storage_config: &StorageConfig,
) -> broadcast::Receiver<RecorderEvent> {
    let (rec_tx, rec_rx) = broadcast::channel::<RecorderEvent>(1024);
    {
        let tx = rec_tx.clone();
        engine
            .hooks_mut()
            .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
                let _ = tx.send(RecorderEvent::Snapshot(snap.clone()));
            });
    }
    {
        let tx = rec_tx.clone();
        engine.hooks_mut().on::<OrderbookUpdate, _>(move |_| {
            let _ = tx.send(RecorderEvent::RawUpdate);
        });
    }
    {
        let tx = rec_tx.clone();
        engine
            .hooks_mut()
            .on::<LastTradePrice, _>(move |trade: &LastTradePrice| {
                let _ = tx.send(RecorderEvent::Trade(trade.clone()));
            });
    }
    drop(rec_tx);

    info!(
        base_path = %storage_config.base_path.display(),
        depth = storage_config.depth,
        "Storage recorder enabled"
    );
    rec_rx
}

// ── Exchange registration ─────────────────────────────────────────────────────

fn add_binance(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&refs)
                .build(),
        ),
    );
    info!(symbols = ?symbols, "Binance enabled");
    true
}

fn add_binance_spot(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::BinanceSpot).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&refs)
                .with_trade_channel(&refs)
                .build(),
        ),
    );
    info!(symbols = ?symbols, "Binance Spot enabled");
    true
}

fn add_okx(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Okx).set_subscription_message(
            OkxSubMsgBuilder::new()
                .with_orderbook_channel_multi(refs, "SPOT")
                .build(),
        ),
    );
    info!(symbols = ?symbols, "OKX enabled");
    true
}

fn add_hyperliquid(cfg: &mut StreamSystemConfig, coins: &[String]) -> bool {
    if coins.is_empty() {
        return false;
    }
    let refs: Vec<&str> = coins.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Hyperliquid)
            .set_subscription_messages(HyperliquidSubMsgBuilder::new().with_coins(&refs).build()),
    );
    info!(coins = ?coins, "Hyperliquid enabled");
    true
}
