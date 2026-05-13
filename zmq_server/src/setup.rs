//! Reusable helpers for wiring exchanges into a `StreamSystem`.
//!
//! Each function takes a config slice and mutates a `StreamSystemConfig`,
//! returning whether it actually registered anything. The binary calls these
//! in sequence then decides whether to call `validate()`.

use crate::topics::bba::BbaPayload;
use libs::configs::{KalshiMarketConfig, PolymarketMarketConfig};
use libs::constants::WS_STATUS_SOCKET;
use libs::credentials::KalshiCredentials;
use libs::endpoints::kalshi::kalshi;
use libs::protocol::{ExchangeName, LastTradePrice, OrderbookSnapshot, UserEvent};
use libs::redis_client::{
    keys::{Events, RedisKey},
    RedisHandle,
};
use orderbook::connection::{ClientConfig, ClientTrait, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::kalshi::{
    run_rolling_scheduler as kalshi_rolling, KalshiSubMsgBuilder, WindowTask as KalshiWindowTask,
};
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::{
    resolve_assets_with_labels, run_rolling_scheduler as polymarket_rolling,
    PolymarketSubMsgBuilder, PolymarketUserSubMsgBuilder, WindowTask as PolymarketWindowTask,
};
use orderbook::types::endpoints::polymarket as poly_endpoints;
use orderbook::types::orderbook::StreamMessage;
use orderbook::{
    PolymarketClient, StreamEngine, StreamEngineBus, StreamEvent, StreamEventSource, StreamSystem,
    StreamSystemConfig,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use libs::credentials::PolymarketCredentials;

use crate::ZmqServer;

// ── Static exchanges ──────────────────────────────────────────────────────────

/// Register Binance (USDT-M futures) symbols into `cfg`. Returns `true` if any were added.
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

/// Register Binance Spot symbols into `cfg`, subscribing to both `@depth20@100ms`
/// and `@trade` streams. Returns `true` if any were added.
pub fn add_binance_spot(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
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

// ── Recorder (durable archive) ────────────────────────────────────────────────

/// Wire a `recorder::StorageWriter` to the running `StreamSystem`. Snapshots,
/// last-trades, and user events flow into daily CSV files under
/// `cfg.base_path`. The recorder runs as a separate broadcast subscriber, so
/// it cannot block the engine or other consumers.
///
/// Layout produced on disk:
///
///   {base}/{exchange}/{symbol}/{YYYY-MM-DD}.csv          — orderbook snapshots
///   {base}/{exchange}/{symbol}/{YYYY-MM-DD}-trades.csv   — last-trade events
///   {base}/events/{YYYY-MM-DD}.csv                       — user events
///
/// Each `UserEvent` row has a `type` column ∈ {`fill`, `order_update`}.
///
/// `cfg` is the executor-/server-side `recorder::StorageConfig`. Pass `None`
/// to disable; the function is a no-op in that case.
pub fn attach_recorder(system: &mut StreamSystem, cfg: Option<recorder::StorageConfig>) {
    let Some(cfg) = cfg else { return };

    // Bridge `StreamEvent` (engine bus) → `RecorderEvent` (recorder bus).
    // A small bounded broadcast is enough; the writer consumes via tokio mpsc
    // semantics under the hood, but uses broadcast::Receiver so we can fan
    // out further later without changing the writer API.
    let (rec_tx, rec_rx) = tokio::sync::broadcast::channel::<recorder::RecorderEvent>(1024);
    let mut engine_rx = system.engine_bus().subscribe();

    let bridge = tokio::spawn(async move {
        loop {
            match engine_rx.recv().await {
                Ok(StreamEvent::OrderbookSnapshot(snap)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Snapshot(snap));
                }
                Ok(StreamEvent::LastTradePrice(trade)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Trade(trade));
                }
                Ok(StreamEvent::User(ev)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::UserEvent(ev));
                }
                Ok(StreamEvent::OrderbookRaw(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("recorder bridge lagged, skipped {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        debug!("recorder bridge exiting");
    });
    system.attach_handle(bridge);

    let base_path = cfg.base_path.display().to_string();
    let writer_handle = recorder::StorageWriter::new(cfg).start(rec_rx);
    system.attach_handle(writer_handle);
    info!("recorder: archive writer attached, base={base_path}");
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
    main_bus: Option<StreamEngineBus>,
) -> Vec<tokio::task::JoinHandle<()>> {
    markets
        .iter()
        .filter(|m| m.enabled)
        .map(|market| {
            let market = market.clone();
            let redis = redis.clone();
            let main_bus = main_bus.clone();
            info!(slug = %market.slug, interval_secs = market.interval_secs, "Polymarket enabled");
            tokio::spawn(async move {
                let base_slug = market.slug.clone();
                let interval_secs = market.interval_secs;
                polymarket_rolling(base_slug.clone(), interval_secs, move |full_slug| {
                    let base_slug = base_slug.clone();
                    let redis = redis.clone();
                    let main_bus = main_bus.clone();
                    async move {
                        spawn_polymarket_window(
                            base_slug,
                            full_slug,
                            depth,
                            interval_secs,
                            redis,
                            snapshot_cap,
                            trade_cap,
                            main_bus,
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
    main_bus: Option<StreamEngineBus>,
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

    // Forward this per-window engine's snapshots + trades onto the main
    // engine bus so ZmqServer (which is attached to the main engine only)
    // publishes them. Without this, Polymarket frames never reach the SUB.
    if let Some(bus) = main_bus.clone() {
        let bus_snap = bus.clone();
        engine
            .hooks_mut()
            .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
                bus_snap.publish(StreamEvent::OrderbookSnapshot(snap.clone()));
            });
        engine
            .hooks_mut()
            .on::<LastTradePrice, _>(move |trade: &LastTradePrice| {
                bus.publish(StreamEvent::LastTradePrice(trade.clone()));
            });
    }

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

// ── Polymarket user channel ───────────────────────────────────────────────────

/// Spawn a single persistent connection to the Polymarket private user
/// channel (`wss://.../ws/user`). The subscription is account-scoped — with
/// no condition-id filter the server delivers fills, order_updates, balance,
/// and position events for every market this account trades on.
///
/// Each `StreamMessage::UserEvent` is forwarded onto `engine_tx` so the main
/// `StreamEngine` re-broadcasts it as `StreamEvent::User`, which the
/// `ZmqServer` user PUB then publishes on `user.polymarket.{kind}` topics.
///
/// `engine_tx` should be `system.message_sender()` from the main `StreamSystem`.
/// Returns the spawned task handle, or `None` if credentials are missing.
pub fn spawn_polymarket_user_channel(
    creds: PolymarketCredentials,
    engine_tx: mpsc::UnboundedSender<StreamMessage>,
) -> Option<tokio::task::JoinHandle<()>> {
    if creds.api_key.is_empty() || creds.secret.is_empty() {
        warn!("Polymarket user channel: api_key/secret missing — skipping");
        return None;
    }

    info!("Polymarket user channel: supervisor starting (account-wide, no market filter)");

    // Outer supervisor — reconnects forever with exponential backoff.
    //
    // `PolymarketClient::run` has its own internal reconnect loop capped at
    // ~10 attempts; when that loop is exhausted (long network outage,
    // Polymarket-side hiccup) it returns an error and the task ends. The
    // user channel is supposed to live for the whole process lifetime, so
    // we wrap it in an outer retry that spins up a fresh client whenever
    // the inner one gives up. Backoff is capped at `MAX_BACKOFF_SECS` so
    // we keep retrying forever but don't hammer Polymarket if it's down.
    //
    // Combined with the application-level "PING" keepalive fix in
    // `PolymarketMessageParser::build_ping`, this should keep the user
    // channel up across multi-day runs.
    const INITIAL_BACKOFF_SECS: u64 = 1;
    const MAX_BACKOFF_SECS: u64 = 60;
    const HEALTHY_RESET_SECS: u64 = 120;

    Some(tokio::spawn(async move {
        let mut backoff = INITIAL_BACKOFF_SECS;
        loop {
            let attempt_start = std::time::Instant::now();
            match run_user_channel_once(&creds, engine_tx.clone()).await {
                Ok(()) => info!("Polymarket user channel: inner client returned cleanly"),
                Err(e) => error!("Polymarket user channel: inner client exited: {e:?}"),
            }
            // If the prior attempt was healthy for a while, reset the backoff.
            // Protects against the case where the WS dies quickly N times
            // then later runs fine for hours.
            if attempt_start.elapsed().as_secs() > HEALTHY_RESET_SECS {
                backoff = INITIAL_BACKOFF_SECS;
            } else {
                backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
            }
            warn!("Polymarket user channel: reconnecting in {}s", backoff);
            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        }
    }))
}

/// One full attempt at running the user channel until the inner
/// `PolymarketClient` gives up. The forwarder task is aborted before
/// returning, so the next attempt starts with a fresh mpsc pair.
async fn run_user_channel_once(
    creds: &PolymarketCredentials,
    engine_tx: mpsc::UnboundedSender<StreamMessage>,
) -> anyhow::Result<()> {
    let passphrase = creds.passphrase.clone().unwrap_or_default();
    // No `with_condition(...)` — server sends events for every market on the
    // account.
    let sub_json = PolymarketUserSubMsgBuilder::new()
        .with_auth(&creds.api_key, &creds.secret, &passphrase)
        .build();

    let cfg = ClientConfig::new(ExchangeName::Polymarket)
        .set_ws_url(poly_endpoints::ws::USER_STREAM)
        .set_subscription_message(sub_json)
        .set_api_credentials(
            creds.api_key.clone(),
            creds.secret.clone(),
            creds.passphrase.clone(),
        );

    let (tx, mut rx) = mpsc::unbounded_channel::<StreamMessage>();
    let control = SystemControl::new();

    let mut conn = PolymarketClient::new(cfg, tx, control.clone());
    let conn_ctrl = control.clone();
    let conn_task = tokio::spawn(async move {
        let res = conn.run().await;
        conn_ctrl.shutdown();
        res
    });

    let forwarder_engine_tx = engine_tx.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match &msg {
                StreamMessage::UserEvent(_) => {
                    if let Err(e) = forwarder_engine_tx.send(msg) {
                        warn!("user channel: engine tx closed, stopping forwarder: {e}");
                        break;
                    }
                }
                StreamMessage::Base(_) => {}
                _ => {} // user channel shouldn't emit orderbook frames; drop defensively
            }
        }
        debug!("Polymarket user-channel forwarder exiting");
    });

    info!("Polymarket user channel: subscribed (account-wide, no market filter)");

    let result: anyhow::Result<()> = match conn_task.await {
        Ok(inner) => inner.map_err(|e| anyhow::anyhow!("{e:?}")),
        Err(join_err) => Err(anyhow::anyhow!("user-channel task panicked: {join_err}")),
    };
    forwarder.abort();
    result
}

// ── Kalshi scheduler ──────────────────────────────────────────────────────────

/// Spawn one rolling-window scheduler per enabled Kalshi series.
/// Returns an empty vec and logs a warning if credentials are missing.
///
/// `redis` (optional) is used to persist the ticker → label mapping so the
/// HTTP backend can render readable titles, and to write per-window snapshots
/// to `ob:kalshi:{ticker}`.
pub fn spawn_kalshi_schedulers(
    markets: &[KalshiMarketConfig],
    depth: usize,
    credentials_path: &str,
    redis: Option<RedisHandle>,
    snapshot_cap: usize,
    trade_cap: usize,
    main_bus: Option<StreamEngineBus>,
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

    // Kalshi 15-min windows are aligned on UTC :00/:15/:30/:45.
    const KALSHI_INTERVAL_SECS: u64 = 900;

    enabled
        .into_iter()
        .map(|market| {
            let creds = creds.clone();
            let redis = redis.clone();
            let main_bus = main_bus.clone();
            info!(series = %market.series, "Kalshi enabled");
            tokio::spawn(async move {
                let series = market.series.clone();
                kalshi_rolling(series.clone(), move |ticker| {
                    let series = series.clone();
                    let creds = creds.clone();
                    let redis = redis.clone();
                    let main_bus = main_bus.clone();
                    async move {
                        spawn_kalshi_window(
                            series,
                            ticker,
                            depth,
                            creds,
                            redis,
                            snapshot_cap,
                            trade_cap,
                            KALSHI_INTERVAL_SECS,
                            main_bus,
                        )
                        .await
                    }
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
    redis: Option<RedisHandle>,
    snapshot_cap: usize,
    trade_cap: usize,
    interval_secs: u64,
    main_bus: Option<StreamEngineBus>,
) -> Option<KalshiWindowTask> {
    let conn_cfg = ClientConfig::new(ExchangeName::Kalshi)
        .set_ws_url(kalshi::ws::PUBLIC_STREAM)
        .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
        .set_api_credentials(creds.api_key, creds.secret, None);

    let mut cfg = StreamSystemConfig::new();
    cfg.with_exchange(conn_cfg);
    cfg.set_snapshot_depth(depth);

    // Persist ticker → {series, ticker, window_start, interval_secs} for the backend.
    if let Some(r) = redis.clone() {
        // Compute window_start by snapping `now` to the previous :00/:15/:30/:45.
        // Pre-start runs ~10s before the current window closes, so `now` is still
        // inside the previous window — the new window starts at `now + early_start`.
        // We instead derive it from the rule: window_start = current_close,
        // i.e. the next 15-min boundary at or after `now + EARLY_START_SECS`.
        let now = libs::time::now_secs();
        let probe = now + 30; // safely inside the new window
        let window_start = (probe / interval_secs) * interval_secs;
        let window_start_s = window_start.to_string();
        let interval_s = interval_secs.to_string();

        // Evict expired prior-window labels for this series. Active overlapping
        // windows clean themselves up via the watchdog — we never touch them here.
        let series_index = RedisKey::kalshi_series_tickers(&series);
        let prior_tickers = r.smembers(&series_index).await;
        for prior_ticker in &prior_tickers {
            if prior_ticker == &ticker {
                continue;
            }
            let prior_label = RedisKey::kalshi_label(prior_ticker);
            let prior_hash = r.hgetall(&prior_label).await;
            let p_ws = prior_hash
                .get("window_start")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let p_iv = prior_hash
                .get("interval_secs")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let is_stale = p_iv == 0 || (p_ws > 0 && p_ws + p_iv <= now);
            if !is_stale {
                continue;
            }
            r.del(&prior_label).await;
            r.srem(RedisKey::KALSHI_LABEL_INDEX, prior_ticker).await;
            r.del(&RedisKey::orderbook("kalshi", prior_ticker)).await;
            r.del(&RedisKey::bba("kalshi", prior_ticker)).await;
            r.del(&RedisKey::snapshots("kalshi", prior_ticker)).await;
            r.del(&RedisKey::trades("kalshi", prior_ticker)).await;
            r.srem(&series_index, prior_ticker).await;
        }

        r.sadd(&series_index, &ticker).await;
        let r2 = r.clone();
        let key = RedisKey::kalshi_label(&ticker);
        let series_clone = series.clone();
        let ticker_clone = ticker.clone();
        tokio::spawn(async move {
            r2.hset_multi(
                &key,
                &[
                    ("series", &series_clone),
                    ("ticker", &ticker_clone),
                    ("window_start", &window_start_s),
                    ("interval_secs", &interval_s),
                ],
            )
            .await;
            r2.sadd(RedisKey::KALSHI_LABEL_INDEX, &ticker_clone).await;
        });
    }

    let control = SystemControl::new();
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, depth);
    let series_for_log = series.clone();
    engine
        .hooks_mut()
        .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
            debug!(
                series = %series_for_log,
                symbol = %snap.symbol,
                seq = snap.sequence,
                bid = ?snap.best_bid,
                ask = ?snap.best_ask,
                "kalshi snapshot"
            );
        });

    // Forward per-window snapshots + trades onto the main engine bus so
    // ZmqServer publishes them (the per-window engine is invisible to it).
    if let Some(bus) = main_bus.clone() {
        let bus_snap = bus.clone();
        engine
            .hooks_mut()
            .on::<OrderbookSnapshot, _>(move |snap: &OrderbookSnapshot| {
                bus_snap.publish(StreamEvent::OrderbookSnapshot(snap.clone()));
            });
        engine
            .hooks_mut()
            .on::<LastTradePrice, _>(move |trade: &LastTradePrice| {
                bus.publish(StreamEvent::LastTradePrice(trade.clone()));
            });
    }

    if let Some(r) = redis.clone() {
        attach_redis(&mut engine, r, snapshot_cap, trade_cap);
    }

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let ticker_log = ticker.clone();
    let cleanup_redis = redis.clone();
    let cleanup_ticker = ticker.clone();
    let cleanup_series = series.clone();

    if let Some(r) = cleanup_redis {
        let watch_ctrl = control.clone();
        tokio::spawn(async move {
            loop {
                if watch_ctrl.is_shutdown() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let series_index = RedisKey::kalshi_series_tickers(&cleanup_series);
            r.del(&RedisKey::kalshi_label(&cleanup_ticker)).await;
            r.srem(RedisKey::KALSHI_LABEL_INDEX, &cleanup_ticker).await;
            r.del(&RedisKey::orderbook("kalshi", &cleanup_ticker)).await;
            r.del(&RedisKey::bba("kalshi", &cleanup_ticker)).await;
            r.del(&RedisKey::snapshots("kalshi", &cleanup_ticker)).await;
            r.del(&RedisKey::trades("kalshi", &cleanup_ticker)).await;
            r.srem(&series_index, &cleanup_ticker).await;
        });
    }

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
                    h3.lpush_capped(&RedisKey::snapshots(&ex3, &sym3), &ob_bytes, snapshot_cap)
                        .await;
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
            let Ok(bytes) = rmp_serde::to_vec_named(trade) else {
                return;
            };
            let h2 = h.clone();
            let exchange = trade.exchange.to_str().to_owned();
            let symbol = trade.symbol.clone();
            tokio::spawn(async move {
                h2.lpush_capped(&RedisKey::trades(&exchange, &symbol), &bytes, trade_cap)
                    .await;
            });
        });
    }

    // User events hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<UserEvent, _>(move |ev| {
            let Ok(payload) = rmp_serde::to_vec_named(ev) else {
                return;
            };
            let h2 = h.clone();
            let ev = ev.clone();
            tokio::spawn(async move {
                match &ev {
                    UserEvent::Fill { .. } => {
                        h2.lpush_capped(Events::FILLS, &payload, h2.event_list_cap)
                            .await;
                    }
                    UserEvent::OrderUpdate { .. } => {
                        h2.lpush_capped(Events::ORDERS, &payload, h2.event_list_cap)
                            .await;
                    }
                }
            });
        });
    }

    info!("Redis integration enabled (snapshot_cap={snapshot_cap}, trade_cap={trade_cap})");
}
