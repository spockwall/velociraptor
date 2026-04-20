//! Reusable helpers for wiring exchanges into a `StreamSystem`.
//!
//! Each function takes a config slice and mutates a `StreamSystemConfig`,
//! returning whether it actually registered anything. The binary calls these
//! in sequence then decides whether to call `validate()`.

use libs::configs::{KalshiMarketConfig, PolymarketMarketConfig};
use libs::constants::WS_STATUS_SOCKET;
use libs::credentials::KalshiCredentials;
use libs::protocol::{ExchangeName, LastTradePrice, OrderbookSnapshot, UserEvent};
use libs::redis_client::{keys::{Events, RedisKey}, RedisHandle};
use crate::topics::bba::BbaPayload;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::kalshi::{
    run_rolling_scheduler as kalshi_rolling, KalshiSubMsgBuilder, WindowTask as KalshiWindowTask,
};
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::{
    resolve_assets_with_labels, run_rolling_scheduler as polymarket_rolling,
    PolymarketSubMsgBuilder, WindowTask as PolymarketWindowTask,
};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use std::sync::Arc;
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
        ClientConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&refs)
                .build(),
        ),
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
        ClientConfig::new(ExchangeName::Okx).set_subscription_message(
            OkxSubMsgBuilder::new()
                .with_orderbook_channel_multi(refs, "SPOT")
                .build(),
        ),
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
    let handle = ZmqServer::new(server_router, server_pub, WS_STATUS_SOCKET)
        .start(Arc::new(system.engine_bus()));
    system.attach_handle(handle);
    info!("ZMQ PUB    {}", server_pub);
    info!("ZMQ ROUTER {}", server_router);
}

// ── Polymarket scheduler ──────────────────────────────────────────────────────

/// Spawn one rolling-window scheduler per enabled Polymarket market.
///
/// `redis` (optional) is used to persist the asset_id → label mapping so the
/// HTTP backend can render readable titles, and to write per-window snapshots
/// to `ob:polymarket:{asset_id}`.
pub fn spawn_polymarket_schedulers(
    markets: &[PolymarketMarketConfig],
    depth: usize,
    redis: Option<RedisHandle>,
    snapshot_cap: usize,
    trade_cap: usize,
) -> Vec<tokio::task::JoinHandle<()>> {
    markets
        .iter()
        .filter(|m| m.enabled)
        .map(|market| {
            let market = market.clone();
            let redis = redis.clone();
            info!(slug = %market.slug, interval_secs = market.interval_secs, "Polymarket enabled");
            tokio::spawn(async move {
                let base_slug = market.slug.clone();
                let interval_secs = market.interval_secs;
                polymarket_rolling(base_slug.clone(), interval_secs, move |full_slug| {
                    let base_slug = base_slug.clone();
                    let redis = redis.clone();
                    async move {
                        spawn_polymarket_window(
                            base_slug,
                            full_slug,
                            depth,
                            interval_secs,
                            redis,
                            snapshot_cap,
                            trade_cap,
                        )
                        .await
                    }
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
    interval_secs: u64,
    redis: Option<RedisHandle>,
    snapshot_cap: usize,
    trade_cap: usize,
) -> Option<PolymarketWindowTask> {
    use libs::configs::PolymarketMarketConfig;

    let single = vec![PolymarketMarketConfig {
        enabled: true,
        slug: full_slug.clone(),
        interval_secs: 0,
    }];
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

    // Persist asset_id → {base_slug, full_slug, side, window_start} for the backend.
    let asset_ids: Vec<String> = labeled.iter().map(|(id, _, _, _)| id.clone()).collect();
    if let Some(r) = redis.clone() {
        // Parse window_start from the trailing timestamp the scheduler embedded
        // in `full_slug` (format: "{base_slug}-{win_start}"). Computing from
        // `now` here is wrong: this runs during pre-start (~10s before the
        // current window ends), so `now / interval * interval` returns the
        // CURRENT window's start, not the new one we're spinning up.
        let window_start = if interval_secs == 0 {
            0u64
        } else {
            full_slug
                .rsplit('-')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    let now = libs::time::now_secs();
                    (now / interval_secs) * interval_secs
                })
        };
        let window_start_s = window_start.to_string();
        let interval_s = interval_secs.to_string();

        // Evict only EXPIRED prior-window labels for this base slug. We can't
        // touch assets from a still-running overlapping window (pre-start sets
        // up the next window before the previous one ends), or the UI will
        // briefly show "no data" for live markets.
        let base_slug_index = RedisKey::polymarket_base_slug_assets(&slug);
        let prior_assets = r.smembers(&base_slug_index).await;
        let now = libs::time::now_secs();
        for prior_id in &prior_assets {
            if asset_ids.contains(prior_id) {
                continue; // same asset re-resolved; will be overwritten below
            }
            // Check the prior label's window timing — only evict if it has
            // already ended. Active windows are owned by their own task and
            // will clean themselves up via `cleanup_assets` when they stop.
            let prior_label = RedisKey::polymarket_label(prior_id);
            let prior_hash = r.hgetall(&prior_label).await;
            let p_ws = prior_hash
                .get("window_start")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let p_iv = prior_hash
                .get("interval_secs")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            // Stale if: missing/zero interval (legacy), or window has ended.
            let is_stale = p_iv == 0 || (p_ws > 0 && p_ws + p_iv <= now);
            if !is_stale {
                continue;
            }
            r.del(&prior_label).await;
            r.srem(RedisKey::POLYMARKET_LABEL_INDEX, prior_id).await;
            r.del(&RedisKey::orderbook("polymarket", prior_id)).await;
            r.del(&RedisKey::bba("polymarket", prior_id)).await;
            r.del(&RedisKey::snapshots("polymarket", prior_id)).await;
            r.del(&RedisKey::trades("polymarket", prior_id)).await;
            r.srem(&base_slug_index, prior_id).await;
        }

        // Register new window's assets under this base_slug index.
        for id in &asset_ids {
            r.sadd(&base_slug_index, id).await;
        }

        for (id, _resolved_base, full, is_up) in &labeled {
            let side = if *is_up { "up" } else { "down" };
            let key = RedisKey::polymarket_label(id);
            let r2 = r.clone();
            let key2 = key.clone();
            let base = slug.clone(); // original base_slug from config
            let full = full.clone();
            let ws = window_start_s.clone();
            let iv = interval_s.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                r2.hset_multi(
                    &key2,
                    &[
                        ("base_slug", &base),
                        ("full_slug", &full),
                        ("side", side),
                        ("window_start", &ws),
                        ("interval_secs", &iv),
                    ],
                )
                .await;
                r2.sadd(RedisKey::POLYMARKET_LABEL_INDEX, &id_clone).await;
            });
        }
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
    engine
        .hooks_mut()
        .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
            debug!(
                exchange = %snap.exchange,
                symbol = %snap.symbol,
                seq = snap.sequence,
                bid = ?snap.best_bid,
                ask = ?snap.best_ask,
                "polymarket snapshot"
            );
        });

    // Attach Redis snapshot/trade writers to the per-window engine so its
    // data flows into Redis (the main engine's hook only sees static exchanges).
    if let Some(r) = redis.clone() {
        attach_redis(&mut engine, r, snapshot_cap, trade_cap);
    }

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let slug_for_cleanup = slug.clone();
    let cleanup_redis = redis.clone();
    let cleanup_assets = asset_ids.clone();

    // Spawn a watchdog task that fires Redis cleanup when shutdown is signaled.
    // We can't put cleanup inline after `system.run().await` because the
    // scheduler's `WindowTask::stop` calls `handle.abort()`, which cancels the
    // task before any await past the cancellation point completes.
    if let Some(r) = cleanup_redis {
        let watch_ctrl = control.clone();
        let watch_slug = slug.clone();
        tokio::spawn(async move {
            // Poll the SystemControl shutdown flag (no async signal exposed).
            loop {
                if watch_ctrl.is_shutdown() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let base_slug_index = RedisKey::polymarket_base_slug_assets(&watch_slug);
            for id in &cleanup_assets {
                r.del(&RedisKey::polymarket_label(id)).await;
                r.srem(RedisKey::POLYMARKET_LABEL_INDEX, id).await;
                r.del(&RedisKey::orderbook("polymarket", id)).await;
                r.del(&RedisKey::bba("polymarket", id)).await;
                r.del(&RedisKey::snapshots("polymarket", id)).await;
                r.del(&RedisKey::trades("polymarket", id)).await;
                r.srem(&base_slug_index, id).await;
            }
        });
    }

    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!(slug = %slug_for_cleanup, "Polymarket system error: {e}");
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
            warn!(
                path = credentials_path,
                "Kalshi credentials file not found — skipping Kalshi"
            );
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
    engine
        .hooks_mut()
        .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
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

// ── Redis attachment ──────────────────────────────────────────────────────────

/// Register engine hooks that write market and account state to Redis.
///
/// - Latest snapshot: `SET ob:{exchange}:{symbol}` (overwritten each tick)
/// - Latest BBA:      `SET bba:{exchange}:{symbol}` (overwritten each tick)
/// - Recent snapshots: `LPUSH snapshots:{exchange}:{symbol}`, capped at `snapshot_cap`
/// - Recent trades:    `LPUSH trades:{exchange}:{symbol}`, capped at `trade_cap`
/// - Account state:    `SET position:*`, `SET balance:*`
/// - Event lists:      `LPUSH events:fills`, `LPUSH events:orders`, capped at `event_list_cap`
///
/// Hooks fire synchronously — all Redis I/O is dispatched via `tokio::spawn`.
pub fn attach_redis(
    engine: &mut StreamEngine,
    handle: RedisHandle,
    snapshot_cap: usize,
    trade_cap: usize,
) {
    // Orderbook snapshot hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
            let exchange = snap.exchange.to_str().to_owned();
            let symbol = snap.symbol.clone();

            // Latest full snapshot (overwrite)
            if let Ok(ob_bytes) = rmp_serde::to_vec_named(snap) {
                let h2 = h.clone();
                let ex2 = exchange.clone();
                let sym2 = symbol.clone();
                let ob2 = ob_bytes.clone();
                tokio::spawn(async move { h2.set_orderbook(&ex2, &sym2, &ob2).await });

                // Recent snapshot list (capped)
                let h3 = h.clone();
                let ex3 = exchange.clone();
                let sym3 = symbol.clone();
                tokio::spawn(async move {
                    h3.lpush_capped(&RedisKey::snapshots(&ex3, &sym3), &ob_bytes, snapshot_cap).await;
                });
            }

            // Latest BBA (overwrite) — reuse BbaPayload from topics::bba
            if let Ok(bba_bytes) = rmp_serde::to_vec_named(&BbaPayload::from(snap)) {
                let h2 = h.clone();
                let ex2 = exchange.clone();
                let sym2 = symbol.clone();
                tokio::spawn(async move { h2.set_bba(&ex2, &sym2, &bba_bytes).await });
            }
        });
    }

    // Last-trade hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<LastTradePrice, _>(move |trade| {
            let Ok(bytes) = rmp_serde::to_vec_named(trade) else { return };
            let h2 = h.clone();
            let exchange = trade.exchange.to_str().to_owned();
            let symbol = trade.symbol.clone();
            tokio::spawn(async move {
                h2.lpush_capped(&RedisKey::trades(&exchange, &symbol), &bytes, trade_cap).await;
            });
        });
    }

    // User events hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<UserEvent, _>(move |ev| {
            let Ok(payload) = rmp_serde::to_vec_named(ev) else { return };
            let h2 = h.clone();
            let ev = ev.clone();
            tokio::spawn(async move {
                match &ev {
                    UserEvent::Position { exchange, symbol, .. } => {
                        h2.set_position(exchange, symbol, &payload).await;
                    }
                    UserEvent::Balance { exchange, asset, .. } => {
                        h2.set_balance(exchange, asset, &payload).await;
                    }
                    UserEvent::Fill { .. } => {
                        h2.lpush_capped(Events::FILLS, &payload, h2.event_list_cap).await;
                    }
                    UserEvent::OrderUpdate { .. } => {
                        h2.lpush_capped(Events::ORDERS, &payload, h2.event_list_cap).await;
                    }
                }
            });
        });
    }

    info!("Redis integration enabled (snapshot_cap={snapshot_cap}, trade_cap={trade_cap})");
}
