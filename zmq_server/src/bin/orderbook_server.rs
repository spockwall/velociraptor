//! Orderbook server binary.
//!
//! Reads all configuration from a YAML file and starts the ZMQ server.
//! Exchange wiring is delegated to `zmq_server::setup`.
//!
//! Storage is handled separately by `orderbook_recorder` — this process
//! only streams data over ZMQ.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin orderbook_server -- --config configs/dev/config.yaml
//! cargo run --bin orderbook_server -- --config configs/prod/config.yaml
//! ```

use anyhow::Result;
use clap::Parser;
use libs;
use libs::configs::Config;
use libs::logging::init_logging;
use orderbook::connection::SystemControl;
use orderbook::StreamEngine;
use tracing::error;
use zmq_server::setup::{
    add_binance, add_binance_spot, add_hyperliquid, add_okx, attach_recorder, attach_redis,
    attach_zmq, build_system, spawn_kalshi_schedulers, spawn_polymarket_schedulers,
    spawn_polymarket_user_channel,
};
use zmq_server::StreamSystemConfig;

#[derive(Parser, Debug)]
#[command(
    name = "orderbook_server",
    about = "Real-time orderbook server — streams live data from exchanges over ZMQ"
)]
struct Args {
    #[arg(long, env = "CONFIG_FILE", default_value = "configs/dev/config.yaml")]
    config: String,

    /// Path to the Polymarket credentials file (a `polymarket:` section). Used
    /// only for the optional user-event channel; absent file/section → skipped.
    #[arg(
        long,
        env = "POLYMARKET_CREDENTIALS_FILE",
        default_value = "credentials/dev/polymarket.yaml"
    )]
    polymarket_credentials: String,

    /// Path to the Kalshi credentials file (a `kalshi:` section). Absent
    /// file/section → Kalshi schedulers run without authenticated WS upgrade.
    #[arg(
        long,
        env = "KALSHI_CREDENTIALS_FILE",
        default_value = "credentials/dev/kalshi.yaml"
    )]
    kalshi_credentials: String,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    // Load config first so `logging:` settings can drive tracing setup.
    let cfg = Config::load(&args.config);
    let _guards = init_logging(
        "server",
        std::path::Path::new(&cfg.logging.dir),
        &cfg.logging.level,
        cfg.logging.json,
    );

    if let Err(e) = run(cfg, &args).await {
        error!("Fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cfg: Config, args: &Args) -> Result<()> {
    // ── Static exchange config ─────────────────────────────────────────────────

    let mut system_cfg = StreamSystemConfig::new();
    system_cfg.set_snapshot_depth(cfg.storage.depth);

    let has_static = [
        cfg.binance.enabled && add_binance(&mut system_cfg, &cfg.binance.symbols),
        cfg.binance_spot.enabled && add_binance_spot(&mut system_cfg, &cfg.binance_spot.symbols),
        cfg.okx.enabled     && add_okx(&mut system_cfg, &cfg.okx.symbols),
        cfg.hyperliquid.enabled && add_hyperliquid(&mut system_cfg, &cfg.hyperliquid.coins),
    ]
    .iter()
    .any(|&v| v);

    let has_polymarket = cfg.polymarket.markets.iter().any(|m| m.enabled);
    let has_kalshi     = cfg.kalshi.market.iter().any(|m| m.enable);

    if !has_static && !has_polymarket && !has_kalshi {
        anyhow::bail!("No exchanges enabled in config");
    }

    // ── Engine + optional Redis hooks ─────────────────────────────────────────

    let mut engine = StreamEngine::new(1024, cfg.storage.depth);

    let mut redis_handle: Option<libs::redis_client::RedisHandle> = None;
    if cfg.redis.enabled {
        match libs::redis_client::RedisHandle::connect(&cfg.redis.url, cfg.redis.event_list_cap).await {
            Ok(handle) => {
                attach_redis(&mut engine, handle.clone(), cfg.redis.snapshot_cap, cfg.redis.trade_cap);
                redis_handle = Some(handle);
            }
            Err(e) => error!("Redis connection failed: {e} — continuing without Redis"),
        }
    }

    // ── Build StreamSystem ────────────────────────────────────────────────────

    let ctrl = SystemControl::new();
    let mut system = build_system(engine, system_cfg, ctrl.clone(), has_static)?;

    // ── ZMQ server ────────────────────────────────────────────────────────────

    attach_zmq(&mut system, &cfg.server.pub_endpoint, &cfg.server.router_endpoint);

    // ── Archive recorder ─────────────────────────────────────────────────────
    // Writes orderbook snapshots, trades, and user events to daily mpack[.zst]
    // files under `cfg.storage.base_path`. Only attached when storage.enabled.

    if cfg.storage.enabled {
        attach_recorder(&mut system, Some(build_recorder_config(&cfg.storage)));
    } else {
        tracing::info!("recorder: storage.enabled=false in config — archive writer skipped");
    }

    // ── Rolling-window schedulers (Polymarket + Kalshi) ───────────────────────

    let _pm  = spawn_polymarket_schedulers(
        &cfg.polymarket.markets,
        cfg.storage.depth,
        redis_handle.clone(),
        cfg.redis.snapshot_cap,
        cfg.redis.trade_cap,
        Some(system.engine_bus()),
    );

    // Polymarket user channel — single persistent WS, account-scoped (no
    // condition-id filter), forwards UserEvents onto the main engine bus so
    // the ZmqServer user PUB publishes them as `user.polymarket.{kind}`.
    let _pm_user = libs::credentials::PolymarketCredentials::try_load(
        &args.polymarket_credentials,
    )
    .and_then(|c| spawn_polymarket_user_channel(c, system.message_sender()));
    if _pm_user.is_none() {
        tracing::info!(
            path = %args.polymarket_credentials,
            "Polymarket user channel: credentials missing or empty — skipping"
        );
    }

    let _kal = spawn_kalshi_schedulers(
        &cfg.kalshi.market,
        cfg.storage.depth,
        &args.kalshi_credentials,
        redis_handle.clone(),
        cfg.redis.snapshot_cap,
        cfg.redis.trade_cap,
        Some(system.engine_bus()),
    );

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

/// Translate the YAML-level `libs::configs::StorageConfig` into the
/// crate-level `recorder::StorageConfig`. Kept here rather than in `libs`
/// or `recorder` so `libs` doesn't need to know about `recorder`'s types.
fn build_recorder_config(s: &libs::configs::StorageConfig) -> recorder::StorageConfig {
    use std::path::PathBuf;
    let rotation = match s.rotation.as_str() {
        "none" | "off" => recorder::RotationPolicy::None,
        _ => recorder::RotationPolicy::Daily,
    };
    let zstd_level: Option<i32> = if s.zstd_level == 0 {
        None
    } else {
        Some(s.zstd_level as i32)
    };
    recorder::StorageConfig {
        base_path: PathBuf::from(&s.base_path),
        depth: s.depth,
        flush_interval_ms: s.flush_interval,
        rotation,
        zstd_level,
    }
}
