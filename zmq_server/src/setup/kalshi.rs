//! Kalshi rolling-window plumbing.
//!
//! Kalshi 15-min markets roll over on UTC :00/:15/:30/:45 boundaries. Unlike
//! Polymarket there's no up/down split — each window has a single ticker —
//! so the forward hook has no per-frame side gate.
//!
//! See [`super::rolling`] for the lifecycle-shape docstring shared with Polymarket.

use libs::configs::KalshiMarketConfig;
use libs::credentials::KalshiCredentials;
use libs::endpoints::kalshi::kalshi;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use libs::redis_client::{keys::RedisKey, RedisHandle};
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::kalshi::{
    run_rolling_scheduler as kalshi_rolling, KalshiSubMsgBuilder, WindowTask as KalshiWindowTask,
};
use orderbook::{StreamEngine, StreamEngineBus, StreamSystem, StreamSystemConfig};
use tracing::{debug, error, info, warn};

/// Spawn one rolling-window scheduler per enabled Kalshi series.
/// Returns an empty vec and logs a warning if credentials are missing.
///
/// `redis` (optional) is used to persist the ticker → label mapping so the
/// HTTP backend can render readable titles, and to write per-window snapshots
/// to `ob:kalshi:{series}` (stable across ticker rollover).
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

    // Compute window_start by snapping `now` to the next 15-min boundary.
    // Pre-start runs ~10s before the current window closes, so `now` is
    // still inside the previous window — adding +30s lands us safely in
    // the new window for the snap. window_start = the next :00/:15/:30/:45.
    let window_start: u64 = if interval_secs == 0 {
        0
    } else {
        let probe = libs::time::now_secs() + 30;
        (probe / interval_secs) * interval_secs
    };

    // Persist ticker → {series, ticker, window_start, interval_secs} for the backend.
    if let Some(r) = redis.clone() {
        let window_start_s = window_start.to_string();
        let interval_s = interval_secs.to_string();

        // Evict expired prior-window labels for this series. Active overlapping
        // windows clean themselves up via their own watchdog. See
        // `rolling::evict_stale_labels` for the "stale" predicate.
        let series_index = RedisKey::kalshi_series_tickers(&series);
        let now = libs::time::now_secs();
        let current_ticker = vec![ticker.clone()];
        super::rolling::evict_stale_labels(
            &r,
            &series_index,
            RedisKey::KALSHI_LABEL_INDEX,
            |id| RedisKey::kalshi_label(id),
            &current_ticker,
            now,
        )
        .await;

        // Defer label registration until the boundary. See
        // `rolling::spawn_deferred_label_registration` for the rationale.
        let entries = vec![super::rolling::LabelRegistration {
            label_key: RedisKey::kalshi_label(&ticker),
            member_id: ticker.clone(),
            fields: vec![
                ("series", series.clone()),
                ("ticker", ticker.clone()),
                ("window_start", window_start_s.clone()),
                ("interval_secs", interval_s.clone()),
            ],
        }];
        super::rolling::spawn_deferred_label_registration(
            r.clone(),
            RedisKey::KALSHI_LABEL_INDEX,
            series_index.clone(),
            entries,
            window_start,
        );
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
    // ZmqServer publishes them on the STATIC topic `kalshi:{series}`, and
    // mirror to Redis under `(kalshi, series)` keys. See rolling::register_*
    // for the gate semantics and rolling::RollingHookCtx for the fields.
    if let Some(bus) = main_bus.clone() {
        super::rolling::register_kalshi_rolling_hooks(
            &mut engine,
            super::rolling::RollingHookCtx {
                bus,
                redis: redis.clone(),
                exchange: "kalshi",
                grouping_slug: series.clone(),
                full_slug: ticker.clone(),
                window_start,
                snapshot_cap,
                trade_cap,
            },
        );
    }

    // Note: no `attach_redis(&mut engine, ...)` here. The global hook
    // keys Redis by `snap.symbol` (= ticker), which would create a stale
    // entry per rolled-off ticker. The per-frame writes above key by
    // `series` instead.

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let ticker_log = ticker.clone();

    // Watchdog: drop the label on shutdown. See rolling::spawn_label_watchdog.
    if let Some(r) = redis.clone() {
        super::rolling::spawn_label_watchdog(
            r,
            control.clone(),
            RedisKey::KALSHI_LABEL_INDEX,
            RedisKey::kalshi_series_tickers(&series),
            vec![ticker.clone()],
            |id| RedisKey::kalshi_label(id),
        );
    }

    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!(ticker = %ticker_log, "Kalshi system error: {e}");
        }
        ctrl.shutdown();
    });

    Some(KalshiWindowTask::new(ticker, control, handle))
}
