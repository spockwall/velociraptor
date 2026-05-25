//! Polymarket rolling-window plumbing + the account-wide user-event channel.
//!
//! Polymarket Up/Down markets roll over on a fixed cadence (e.g. every 5 or
//! 15 minutes). Each window has two unique asset_ids — UP and DOWN — that are
//! economic mirrors of the same book (buying YES at p = selling NO at 1-p).
//! We subscribe both WS streams so the per-window orderbook engine has full
//! materialisation, but only forward the UP side onto the ZMQ bus + Redis;
//! the DOWN side is dropped at the forward hook.
//!
//! See [`super::rolling`] for the lifecycle-shape docstring shared with Kalshi.

use libs::configs::PolymarketMarketConfig;
use libs::credentials::PolymarketCredentials;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use libs::redis_client::{keys::RedisKey, RedisHandle};
use orderbook::connection::{ClientConfig, ClientTrait, SystemControl};
use orderbook::exchanges::polymarket::{
    PolymarketLabeledAsset, resolve_assets_with_labels,
    run_rolling_scheduler as polymarket_rolling, PolymarketSubMsgBuilder,
    PolymarketUserSubMsgBuilder, WindowTask as PolymarketWindowTask,
};
use orderbook::types::endpoints::polymarket as poly_endpoints;
use orderbook::types::orderbook::StreamMessage;
use orderbook::{
    PolymarketClient, StreamEngine, StreamEngineBus, StreamSystem, StreamSystemConfig,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Spawn one rolling-window scheduler per enabled Polymarket market.
///
/// `redis` (optional) is used to persist the asset_id → label mapping so the
/// HTTP backend can render readable titles, and to write per-window snapshots
/// to `ob:polymarket:{base_slug}` (the stable, rollover-safe key).
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
                polymarket_rolling(
                    base_slug.clone(),
                    interval_secs,
                    move |full_slug, prefetched| {
                        let base_slug = base_slug.clone();
                        let redis = redis.clone();
                        let main_bus = main_bus.clone();
                        async move {
                            spawn_polymarket_window(
                                base_slug,
                                full_slug,
                                prefetched,
                                depth,
                                interval_secs,
                                redis,
                                snapshot_cap,
                                trade_cap,
                                main_bus,
                            )
                            .await
                        }
                    },
                )
                .await;
            })
        })
        .collect()
}

async fn spawn_polymarket_window(
    slug: String,
    full_slug: String,
    prefetched: Option<Vec<PolymarketLabeledAsset>>,
    depth: usize,
    interval_secs: u64,
    redis: Option<RedisHandle>,
    snapshot_cap: usize,
    trade_cap: usize,
    main_bus: Option<StreamEngineBus>,
) -> Option<PolymarketWindowTask> {
    // Use the scheduler's prefetched tokens when available; otherwise
    // fall back to the inline blocking REST resolve (cold start, or
    // prefetch failed and we hit the retry path).
    let labeled = if let Some(p) = prefetched {
        p
    } else {
        let single = vec![PolymarketMarketConfig {
            enabled: true,
            slug: full_slug.clone(),
            interval_secs: 0,
        }];
        tokio::task::spawn_blocking({
            let single = single.clone();
            move || resolve_assets_with_labels(&single)
        })
        .await
        .ok()?
    };

    if labeled.is_empty() {
        warn!(full_slug = %full_slug, "No tokens resolved — market not open yet?");
        return None;
    }

    // Parse window_start from the trailing timestamp the scheduler embedded
    // in `full_slug` (format: "{base_slug}-{win_start}"). Computing from
    // `now` here is wrong: this runs during pre-start (~10s before the
    // current window ends), so `now / interval * interval` returns the
    // CURRENT window's start, not the new one we're spinning up.
    let window_start: u64 = if interval_secs == 0 {
        0
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

    // Persist asset_id → {base_slug, full_slug, side, window_start} for the backend.
    let asset_ids: Vec<String> = labeled.iter().map(|(id, _, _, _)| id.clone()).collect();
    if let Some(r) = redis.clone() {
        let window_start_s = window_start.to_string();
        let interval_s = interval_secs.to_string();

        // Evict only EXPIRED prior-window labels for this base slug. Active
        // overlapping windows (pre-start spins up the next window ~10s before
        // the previous one ends) are left alone — they clean themselves up
        // via their own watchdog. See `rolling::evict_stale_labels` for the
        // "stale" predicate.
        let base_slug_index = RedisKey::polymarket_base_slug_assets(&slug);
        let now = libs::time::now_secs();
        super::rolling::evict_stale_labels(
            &r,
            &base_slug_index,
            RedisKey::POLYMARKET_LABEL_INDEX,
            |id| RedisKey::polymarket_label(id),
            &asset_ids,
            now,
        )
        .await;

        // Register the new window's labels at the boundary (not now). See
        // `rolling::spawn_deferred_label_registration` for the rationale.
        let entries: Vec<super::rolling::LabelRegistration> = labeled
            .iter()
            .map(|(id, _b, full, is_up)| {
                let side = if *is_up { "up" } else { "down" };
                super::rolling::LabelRegistration {
                    label_key: RedisKey::polymarket_label(id),
                    member_id: id.clone(),
                    fields: vec![
                        ("base_slug", slug.clone()),
                        ("full_slug", full.clone()),
                        ("side", side.to_string()),
                        ("window_start", window_start_s.clone()),
                        ("interval_secs", interval_s.clone()),
                    ],
                }
            })
            .collect();
        super::rolling::spawn_deferred_label_registration(
            r.clone(),
            RedisKey::POLYMARKET_LABEL_INDEX,
            base_slug_index.clone(),
            entries,
            window_start,
        );
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
    // engine bus so ZmqServer publishes them on the STATIC topic
    // `polymarket:{base_slug}`, and mirror to Redis under `(polymarket,
    // base_slug)` keys. The Polymarket variant gates each frame on
    // `is_up_by_asset.get(&snap.symbol)` — UP-only forwarding because
    // Up/Down tokens are mirror images of the same book.
    //
    // Note: we DO NOT call `attach_redis(&mut engine, ...)` here. That
    // generic hook keys Redis by `snap.symbol` (= asset_id), which under
    // the static-topic design would create one stale entry per rolled-off
    // asset. The forward hook keys writes by base_slug instead.
    if let Some(bus) = main_bus.clone() {
        // asset_id → is_up. `labeled` is (asset_id, resolved_base, full_slug, is_up).
        let is_up_by_asset: std::collections::HashMap<String, bool> = labeled
            .iter()
            .map(|(id, _b, _f, is_up)| (id.clone(), *is_up))
            .collect();
        super::rolling::register_polymarket_rolling_hooks(
            &mut engine,
            super::rolling::RollingHookCtx {
                bus,
                redis: redis.clone(),
                exchange: "polymarket",
                grouping_slug: slug.clone(),
                full_slug: full_slug.clone(),
                window_start,
                snapshot_cap,
                trade_cap,
            },
            is_up_by_asset,
        );
    }

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let slug_for_cleanup = slug.clone();

    // Watchdog: drop labels on shutdown. See rolling::spawn_label_watchdog
    // for why this can't live inline after system.run().await.
    if let Some(r) = redis.clone() {
        super::rolling::spawn_label_watchdog(
            r,
            control.clone(),
            RedisKey::POLYMARKET_LABEL_INDEX,
            RedisKey::polymarket_base_slug_assets(&slug),
            asset_ids.clone(),
            |id| RedisKey::polymarket_label(id),
        );
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
