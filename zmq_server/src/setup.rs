//! Reusable helpers for wiring exchanges into a `StreamSystem`.
//!
//! Each function takes a config slice and mutates a `StreamSystemConfig`,
//! returning whether it actually registered anything. The binary calls these
//! in sequence then decides whether to call `validate()`.

use libs::configs::{Config, KalshiMarketConfig, PolymarketMarketConfig};
use libs::constants::WS_STATUS_SOCKET;
use libs::credentials::KalshiCredentials;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::kalshi::{
    KalshiSubMsgBuilder, WindowTask as KalshiWindowTask, run_rolling_scheduler as kalshi_rolling,
};
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::{
    PolymarketSubMsgBuilder, WindowTask as PolymarketWindowTask, resolve_assets_with_labels,
    run_rolling_scheduler as polymarket_rolling,
};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use recorder::{RecorderEvent, RotationPolicy, StorageConfig, StorageWriter};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::ZmqServer;

// ── Static exchanges ──────────────────────────────────────────────────────────

/// Register Binance symbols into `cfg`. Returns `true` if any were added.
pub fn add_binance(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Binance)
            .set_subscription_message(BinanceSubMsgBuilder::new().with_orderbook_channel(&refs).build()),
    );
    info!(symbols = ?symbols, "Binance enabled");
    true
}

/// Register OKX symbols into `cfg`. Returns `true` if any were added.
pub fn add_okx(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Okx)
            .set_subscription_message(OkxSubMsgBuilder::new().with_orderbook_channel_multi(refs, "SPOT").build()),
    );
    info!(symbols = ?symbols, "OKX enabled");
    true
}

/// Register Hyperliquid coins into `cfg`. Returns `true` if any were added.
pub fn add_hyperliquid(cfg: &mut StreamSystemConfig, coins: &[String]) -> bool {
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

// ── Storage recorder ──────────────────────────────────────────────────────────

/// Attach storage hooks to `engine` and return the writer handle.
/// Must be called before the engine is consumed by `StreamSystem::new`.
pub fn attach_storage(
    engine: &mut StreamEngine,
    server_cfg: &Config,
) -> Option<(StorageConfig, broadcast::Receiver<RecorderEvent>)> {
    if !server_cfg.storage.enabled {
        return None;
    }
    let rotation = match server_cfg.storage.rotation.as_str() {
        "none" => RotationPolicy::None,
        _ => RotationPolicy::Daily,
    };
    let zstd_level = match server_cfg.storage.zstd_level {
        0 => None,
        l => Some(l as i32),
    };
    let storage_config = StorageConfig {
        base_path: server_cfg.storage.base_path.clone().into(),
        depth: server_cfg.storage.depth,
        flush_interval_ms: server_cfg.storage.flush_interval,
        rotation,
        zstd_level,
    };
    info!(
        base_path = %storage_config.base_path.display(),
        depth = storage_config.depth,
        "Storage recorder enabled"
    );

    let (rec_tx, rec_rx) = broadcast::channel::<RecorderEvent>(1024);
    {
        let tx = rec_tx.clone();
        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
            let _ = tx.send(RecorderEvent::Snapshot(snap.clone()));
        });
    }
    {
        use orderbook::types::orderbook::OrderbookUpdate;
        let tx = rec_tx.clone();
        engine.hooks_mut().on::<OrderbookUpdate, _>(move |_| {
            let _ = tx.send(RecorderEvent::RawUpdate);
        });
    }
    drop(rec_tx);
    Some((storage_config, rec_rx))
}

// ── StreamSystem bootstrap ────────────────────────────────────────────────────

/// Build the main `StreamSystem` from `engine` + `system_cfg`.
///
/// If `has_static` is false a dummy no-op exchange is inserted so the engine
/// starts and the ZMQ bus is available for Polymarket/Kalshi windows.
pub fn build_system(
    engine: StreamEngine,
    mut system_cfg: StreamSystemConfig,
    ctrl: SystemControl,
    has_static: bool,
) -> anyhow::Result<StreamSystem> {
    if has_static {
        system_cfg.validate()?;
    } else {
        system_cfg.with_exchange(
            ClientConfig::new(ExchangeName::Binance).set_subscription_message(String::new()),
        );
    }
    Ok(StreamSystem::new(engine, system_cfg, ctrl)?)
}

// ── ZMQ server ────────────────────────────────────────────────────────────────

/// Start the ZMQ server and attach its handle to `system`.
pub fn attach_zmq(system: &mut StreamSystem, server_pub: &str, server_router: &str) {
    let handle = ZmqServer::new(server_pub, server_router, WS_STATUS_SOCKET)
        .start(Arc::new(system.engine_bus()));
    system.attach_handle(handle);
    info!("ZMQ PUB    {}", server_pub);
    info!("ZMQ ROUTER {}", server_router);
}

// ── Polymarket scheduler ──────────────────────────────────────────────────────

/// Spawn one rolling-window scheduler per enabled Polymarket market.
pub fn spawn_polymarket_schedulers(
    markets: &[PolymarketMarketConfig],
    depth: usize,
) -> Vec<tokio::task::JoinHandle<()>> {
    markets
        .iter()
        .filter(|m| m.enabled)
        .map(|market| {
            let market = market.clone();
            info!(slug = %market.slug, interval_secs = market.interval_secs, "Polymarket enabled");
            tokio::spawn(async move {
                let base_slug = market.slug.clone();
                polymarket_rolling(base_slug.clone(), market.interval_secs, move |full_slug| {
                    let base_slug = base_slug.clone();
                    async move { spawn_polymarket_window(base_slug, full_slug, depth).await }
                })
                .await;
            })
        })
        .collect()
}

async fn spawn_polymarket_window(
    slug: String,
    full_slug: String,
    depth: usize,
) -> Option<PolymarketWindowTask> {
    use libs::configs::PolymarketMarketConfig;

    let single = vec![PolymarketMarketConfig { enabled: true, slug: full_slug.clone(), interval_secs: 0 }];
    let labeled = tokio::task::spawn_blocking({
        let single = single.clone();
        move || resolve_assets_with_labels(&single)
    })
    .await
    .ok()?;

    if labeled.is_empty() {
        warn!(full_slug = %full_slug, "No tokens resolved — market not open yet?");
        return None;
    }

    let mut builder = PolymarketSubMsgBuilder::new();
    for (id, _, _, _) in &labeled {
        builder = builder.with_asset(id);
    }

    let mut cfg = StreamSystemConfig::new();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Polymarket).set_subscription_message(builder.build()),
    );
    cfg.validate().ok()?;

    let control = SystemControl::new();
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, depth);
    engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
        debug!(
            exchange = %snap.exchange,
            symbol = %snap.symbol,
            seq = snap.sequence,
            bid = ?snap.best_bid,
            ask = ?snap.best_ask,
            "polymarket snapshot"
        );
    });

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!(slug = %slug, "Polymarket system error: {e}");
        }
        ctrl.shutdown();
    });

    info!(full_slug = %full_slug, "Polymarket window started");
    Some(PolymarketWindowTask::new(full_slug, control, handle))
}

// ── Kalshi scheduler ──────────────────────────────────────────────────────────

/// Spawn one rolling-window scheduler per enabled Kalshi series.
/// Returns an empty vec and logs a warning if credentials are missing.
pub fn spawn_kalshi_schedulers(
    markets: &[KalshiMarketConfig],
    depth: usize,
    credentials_path: &str,
) -> Vec<tokio::task::JoinHandle<()>> {
    let enabled: Vec<_> = markets.iter().filter(|m| m.enable).cloned().collect();
    if enabled.is_empty() {
        return vec![];
    }

    let creds = match std::fs::metadata(credentials_path) {
        Ok(_) => KalshiCredentials::load(credentials_path),
        Err(_) => {
            warn!(path = credentials_path, "Kalshi credentials file not found — skipping Kalshi");
            return vec![];
        }
    };

    enabled
        .into_iter()
        .map(|market| {
            let creds = creds.clone();
            info!(series = %market.series, "Kalshi enabled");
            tokio::spawn(async move {
                let series = market.series.clone();
                kalshi_rolling(series.clone(), move |ticker| {
                    let series = series.clone();
                    let creds = creds.clone();
                    async move { spawn_kalshi_window(series, ticker, depth, creds).await }
                })
                .await;
            })
        })
        .collect()
}

async fn spawn_kalshi_window(
    series: String,
    ticker: String,
    depth: usize,
    creds: KalshiCredentials,
) -> Option<KalshiWindowTask> {
    let conn_cfg = ClientConfig::new(ExchangeName::Kalshi)
        .set_ws_url(creds.ws_url())
        .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
        .set_api_credentials(creds.api_key, creds.secret, None);

    let mut cfg = StreamSystemConfig::new();
    cfg.with_exchange(conn_cfg);
    cfg.set_snapshot_depth(depth);

    let control = SystemControl::new();
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, depth);
    engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
        debug!(
            series = %series,
            symbol = %snap.symbol,
            seq = snap.sequence,
            bid = ?snap.best_bid,
            ask = ?snap.best_ask,
            "kalshi snapshot"
        );
    });

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let ticker_log = ticker.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!(ticker = %ticker_log, "Kalshi system error: {e}");
        }
        ctrl.shutdown();
    });

    Some(KalshiWindowTask::new(ticker, control, handle))
}

// ── StorageWriter attachment ──────────────────────────────────────────────────

/// Start the `StorageWriter` and attach it to `system`.
pub fn attach_storage_writer(
    system: &mut StreamSystem,
    setup: Option<(StorageConfig, broadcast::Receiver<RecorderEvent>)>,
) {
    if let Some((storage_config, rec_rx)) = setup {
        system.attach_handle(StorageWriter::new(storage_config).start(rec_rx));
    }
}
