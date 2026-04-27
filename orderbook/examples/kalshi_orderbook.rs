//! Kalshi 15-minute orderbook visualizer with auto-rotating scheduler.
//!
//! Kalshi runs rolling 15-minute binary markets. Each market has a ticker like
//! `KXBTC15M-26APR130815-15` where `KXBTC15M` is the series and `26APR130815`
//! encodes the window **close** time in US Eastern Time.
//!
//! The scheduler (`orderbook::exchanges::kalshi::run_rolling_scheduler`) owns
//! the 15-min rotation loop. This example only provides `spawn_market_task` —
//! the per-window connection logic — and the render loop.
//!
//! Reads markets from `configs/kalshi.yaml` and credentials from
//! `credentials/kalshi.yaml`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example kalshi_orderbook
//! ```

use anyhow::Result;
use chrono::Utc;
use libs::configs::{Config, KalshiMarketConfig};
use libs::credentials::KalshiCredentials;
use libs::endpoints::kalshi::kalshi;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use libs::terminal::PolymarketUi;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::kalshi::{
    KalshiSubMsgBuilder, WindowTask, build_event_ticker, current_window_close,
    run_rolling_scheduler,
};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CONFIG_PATH: &str = "configs/kalshi.yaml";
const CREDENTIALS_PATH: &str = "credentials/kalshi.yaml";
const DEPTH: usize = 20;
const RENDER_INTERVAL_MS: u64 = 300;

// ── Shared snapshot store ─────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct MarketSide {
    sequence: u64,
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
    spread: Option<f64>,
    mid: Option<f64>,
}

/// Latest orderbook snapshot for one series (e.g. `"KXBTC15M"`).
/// Keyed by series; the label tracks the current ticker across rotations.
#[derive(Clone, Default)]
struct MarketSnap {
    display_label: String,
    yes: MarketSide,
    no: MarketSide,
}

type SnapStore = Arc<Mutex<HashMap<String, MarketSnap>>>;

// ── Per-window market task ────────────────────────────────────────────────────

/// Spawn one orderbook connection for `ticker`. Returns a `WindowTask` the
/// scheduler will manage, or `None` if the system could not be built.
async fn spawn_market_task(
    series: String,
    ticker: String,
    store: SnapStore,
    depth: usize,
    creds: KalshiCredentials,
) -> Option<WindowTask> {
    let conn_cfg = ClientConfig::new(ExchangeName::Kalshi)
        .set_ws_url(kalshi::BASE_URL)
        .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
        .set_api_credentials(creds.api_key, creds.secret, None);

    let mut cfg = StreamSystemConfig::new();
    cfg.with_exchange(conn_cfg);
    if cfg.validate().is_err() {
        return None;
    }

    let control = SystemControl::new();
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, depth);

    let store_w = store.clone();
    let series_ev = series.clone();
    let ticker_ev = ticker.clone();
    engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
        let bids: Vec<(f64, f64)> = snap.bids.iter().take(depth).copied().collect();
        let asks: Vec<(f64, f64)> = snap.asks.iter().take(depth).copied().collect();
        let Ok(mut map) = store_w.lock() else { return };
        let entry = map.entry(series_ev.clone()).or_default();
        // Drop stale callbacks from the previous window.
        if !entry.display_label.is_empty() && entry.display_label != ticker_ev {
            return;
        }
        entry.display_label = ticker_ev.clone();
        entry.yes = MarketSide {
            sequence: snap.sequence,
            bids: bids.clone(),
            asks: asks.clone(),
            spread: snap.spread,
            mid: snap.mid,
        };
        // NO panel: binary-market complement — no_bid(p) = 1 - yes_ask(p).
        entry.no = MarketSide {
            sequence: snap.sequence,
            bids: asks.iter().map(|(p, q)| (1.0 - p, *q)).collect(),
            asks: bids.iter().map(|(p, q)| (1.0 - p, *q)).collect(),
            spread: snap.spread,
            mid: snap.mid.map(|m| 1.0 - m),
        };
    });

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            eprintln!("[market-task] {series} error: {e}");
        }
        ctrl.shutdown();
    });

    eprintln!("[scheduler] Started task for {ticker}");
    Some(WindowTask::new(ticker, control, handle))
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

fn spawn_scheduler(
    market: KalshiMarketConfig,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    depth: usize,
    creds: KalshiCredentials,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let series = market.series.clone();

        // Pre-create the UI panel so it appears immediately.
        if let Ok(mut ui) = ui.lock() {
            ui.ensure(&series);
        }

        run_rolling_scheduler(series.clone(), move |ticker| {
            let series = series.clone();
            let store = store.clone();
            let creds = creds.clone();
            async move {
                // Keep display_label in sync so the staleness guard in the hook
                // lets this window's snapshots through after a rotation swap.
                if let Ok(mut map) = store.lock() {
                    map.entry(series.clone()).or_default().display_label = ticker.clone();
                }
                spawn_market_task(series, ticker, store, depth, creds).await
            }
        })
        .await;
    })
}

// ── Render loop ───────────────────────────────────────────────────────────────

fn spawn_render_loop(
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            let snaps: Vec<(String, MarketSnap)> = {
                let Ok(map) = store.lock() else { continue };
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            let Ok(mut ui) = ui.lock() else { continue };
            for (series, snap) in snaps {
                let lbl = &snap.display_label;
                ui.set_side(
                    &series,
                    lbl,
                    true,
                    snap.yes.sequence,
                    snap.yes.bids,
                    snap.yes.asks,
                    snap.yes.spread,
                    snap.yes.mid,
                );
                ui.set_side(
                    &series,
                    lbl,
                    false,
                    snap.no.sequence,
                    snap.no.bids,
                    snap.no.asks,
                    snap.no.spread,
                    snap.no.mid,
                );
            }
            ui.render();
        }
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer(std::io::sink)
        .try_init();

    let creds = KalshiCredentials::load(CREDENTIALS_PATH);
    let markets: Vec<KalshiMarketConfig> = Config::load(CONFIG_PATH)
        .kalshi
        .market
        .into_iter()
        .filter(|m| m.enable)
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {CONFIG_PATH}");
        std::process::exit(1);
    }

    let now = Utc::now();
    let win_close = current_window_close(now);
    eprintln!(
        "Current 15-min window closes: {} UTC",
        win_close.format("%Y-%m-%d %H:%M")
    );
    for m in &markets {
        eprintln!(
            "  {} → {}",
            m.series,
            build_event_ticker(&m.series, win_close)
        );
    }

    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(DEPTH)));

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|m| spawn_scheduler(m, store.clone(), ui.clone(), DEPTH, creds.clone()))
        .collect();

    let _render = spawn_render_loop(store, ui, Duration::from_millis(RENDER_INTERVAL_MS));

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down.");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orderbook::exchanges::kalshi::{current_window_start, format_ticker_dt};

    #[test]
    fn ticker_uses_eastern_time() {
        let utc = Utc.with_ymd_and_hms(2026, 4, 15, 14, 45, 0).unwrap();
        assert_eq!(format_ticker_dt(utc), "26APR151045");
        assert_eq!(build_event_ticker("KXBTC15M", utc), "KXBTC15M-26APR151045");
        assert_eq!(build_event_ticker("KXETH15M", utc), "KXETH15M-26APR151045");
    }

    #[test]
    fn ticker_winter_est_offset() {
        let utc = Utc.with_ymd_and_hms(2026, 1, 13, 10, 30, 0).unwrap();
        assert_eq!(format_ticker_dt(utc), "26JAN130530");
    }

    #[test]
    fn window_boundaries() {
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 2, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 0);
        assert_eq!(current_window_close(t).minute(), 15);

        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 15, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 15);
        assert_eq!(current_window_close(t).minute(), 30);

        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 59, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 45);
        let close = current_window_close(t);
        assert_eq!(close.hour(), 9);
        assert_eq!(close.minute(), 0);
    }
}
