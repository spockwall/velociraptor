//! Kalshi 15-minute BTC/ETH orderbook visualizer with auto-rotating scheduler.
//!
//! Kalshi runs rolling 15-minute binary markets for BTC and ETH price direction.
//! Each market has a ticker like `KXBTC15M-26APR130815-15` where:
//! - `KXBTC15M` = series (15-min BTC)
//! - `26APR130815` = window **close** time in US Eastern Time (YYMONDDHHММ)
//!
//! The scheduler computes the current and next ticker deterministically from
//! the wall clock. It connects to both BTC and ETH simultaneously, and
//! rotates each market independently:
//! - `EARLY_START_SECS` before the current window closes, it opens the next
//!   window's connection.
//! - When the current window expires, it shuts down the old connection.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example kalshi_orderbook
//! ```
//!
//! Reads markets from `configs/kalshi.yaml` and credentials from
//! `credentials/kalshi.yaml`.

use anyhow::Result;
use chrono::{Duration, Utc};
use libs::configs::{Config, KalshiMarketConfig};
use libs::credentials::KalshiCredentials;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use libs::terminal::PolymarketUi;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::kalshi::{
    KalshiSubMsgBuilder, build_event_ticker, build_market_ticker, current_window_close,
};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

const CONFIG_PATH: &str = "configs/kalshi.yaml";
const CREDENTIALS_PATH: &str = "credentials/kalshi.yaml";

const DEPTH: usize = 20;
const RENDER_INTERVAL_MS: u64 = 300;
const EARLY_START_SECS: u64 = 10;

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
/// The key stays constant across window rotations so the UI panel is stable.
#[derive(Clone, Default)]
struct MarketSnap {
    display_label: String, // full ticker shown in the panel title
    yes: MarketSide,       // YES / bids (rising price contracts)
    no: MarketSide,        // NO  / asks (falling price contracts)
}

type SnapStore = Arc<Mutex<HashMap<String, MarketSnap>>>;

// ── Market task (one window connection) ───────────────────────────────────────

struct MarketTask {
    ticker: String,
    system_control: SystemControl,
    handle: tokio::task::JoinHandle<()>,
}

impl MarketTask {
    async fn spawn(
        series: String,
        ticker: String,
        store: SnapStore,
        depth: usize,
        creds: KalshiCredentials,
    ) -> Option<Self> {
        let conn_cfg = ClientConfig::new(ExchangeName::Kalshi)
            .set_ws_url(creds.ws_url())
            .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
            .set_api_credentials(creds.api_key, creds.secret, None);

        let mut cfg = StreamSystemConfig::new();
        cfg.with_exchange(conn_cfg);
        if cfg.validate().is_err() {
            return None;
        }

        let control = SystemControl::new();
        let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, 20);

        let store_w = store.clone();
        let series_ev = series.clone();
        let ticker_ev = ticker.clone();

        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
            let bids: Vec<(f64, f64)> = snap.bids.iter().take(depth).copied().collect();
            let asks: Vec<(f64, f64)> = snap.asks.iter().take(depth).copied().collect();
            let spread = snap.spread;
            let mid = snap.mid;
            let seq = snap.sequence;
            let Ok(mut map) = store_w.lock() else { return };
            let entry = map.entry(series_ev.clone()).or_default();
            // Don't stomp newer-window data. Another MarketTask may have
            // already pre-written the next ticker's label during rotation;
            // in that case this callback is stale and should bail.
            if !entry.display_label.is_empty() && entry.display_label != ticker_ev {
                return;
            }
            entry.display_label = ticker_ev.clone();

            // YES panel: book as-is.
            entry.yes = MarketSide {
                sequence: seq,
                bids: bids.clone(),
                asks: asks.clone(),
                spread,
                mid,
            };
            // NO panel: binary-market mirror — no_bid(p) = 1 - yes_ask(p),
            // no_ask(p) = 1 - yes_bid(p). Reverse the level order too so the
            // best NO bid (highest price) is on top after the complement.
            let no_bids: Vec<(f64, f64)> = asks.iter().map(|(p, q)| (1.0 - p, *q)).collect();
            let no_asks: Vec<(f64, f64)> = bids.iter().map(|(p, q)| (1.0 - p, *q)).collect();
            let no_mid = mid.map(|m| 1.0 - m);
            entry.no = MarketSide {
                sequence: seq,
                bids: no_bids,
                asks: no_asks,
                spread,
                mid: no_mid,
            };
        });

        let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;

        let ctrl = control.clone();
        let handle = tokio::spawn(async move {
            let _ = system.run().await;
            ctrl.shutdown();
        });

        Some(Self {
            ticker,
            system_control: control,
            handle,
        })
    }

    fn stop(self) {
        self.system_control.shutdown();
        self.handle.abort();
    }
}

// ── Rolling scheduler (one per series) ───────────────────────────────────────

/// Runs the window-rotation loop for one series (e.g. `KXBTC15M`).
///
/// Loop:
/// 1. Compute the start/close times of the current 15-min window → build ticker.
/// 2. Connect if not already running for this window.
/// 3. Sleep until `early_start_secs` before the window closes.
/// 4. Pre-start the next window's connection.
/// 5. When the current window expires, drop the old connection.
/// 6. Repeat.
fn spawn_scheduler(
    market: KalshiMarketConfig,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    depth: usize,
    early_start_secs: u64,
    creds: KalshiCredentials,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let s = market.series.clone();

        // Pre-create panel so it appears immediately.
        if let Ok(mut ui) = ui.lock() {
            ui.ensure(&s);
        }

        let mut current: Option<MarketTask> = None;

        loop {
            let win_close = current_window_close(Utc::now());
            let win_close_unix = win_close.timestamp() as u64;

            // Strike suffix = close-time minute in ET; compute locally.
            let ticker = build_market_ticker(&s, win_close);

            let needs_connect = current.as_ref().map(|t| t.ticker != ticker).unwrap_or(true);
            if needs_connect {
                if let Some(old) = current.take() {
                    old.stop();
                }
                current = MarketTask::spawn(
                    s.clone(),
                    ticker.clone(),
                    store.clone(),
                    depth,
                    creds.clone(),
                )
                .await;
            }

            // Update display_label every iteration. After a rotation swap
            // (`current = next_task`), `needs_connect` is false but the
            // display_label still points at the *previous* window's ticker.
            // The on_update staleness guard compares incoming ticker against
            // display_label and drops mismatches — so without this update, the
            // newly-promoted task's snapshots are silently discarded and the
            // UI freezes on the old window.
            if let Ok(mut map) = store.lock() {
                map.entry(s.clone()).or_default().display_label = ticker.clone();
            }

            // Sample `now` fresh so the WS handshake inside `spawn()` is accounted for.
            let secs_until_early = win_close_unix
                .saturating_sub(Utc::now().timestamp() as u64)
                .saturating_sub(early_start_secs);
            if secs_until_early > 0 {
                tokio::time::sleep(StdDuration::from_secs(secs_until_early)).await;
            }

            // Pre-start next window — strike suffix computed locally.
            let next_close = win_close + Duration::minutes(15);
            let next_ticker = build_market_ticker(&s, next_close);
            let next_task =
                MarketTask::spawn(s.clone(), next_ticker, store.clone(), depth, creds.clone())
                    .await;

            // waiting for current window fully closed
            let remaining = win_close_unix.saturating_sub(Utc::now().timestamp() as u64);
            if remaining > 0 {
                tokio::time::sleep(StdDuration::from_secs(remaining)).await;
            }

            // Swap: drop current, promote next. Also flip display_label to the
            // promoted ticker immediately so the staleness guard in on_update
            // stops dropping its snapshots.
            if let Some(old) = current.take() {
                old.stop();
            }
            if let Some(nt) = &next_task {
                if let Ok(mut map) = store.lock() {
                    map.entry(s.clone()).or_default().display_label = nt.ticker.clone();
                }
            }
            current = next_task;
        }
    })
}

// ── Render loop ───────────────────────────────────────────────────────────────

fn spawn_render_loop(
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    interval: StdDuration,
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
                    snap.yes.bids.clone(),
                    snap.yes.asks.clone(),
                    snap.yes.spread,
                    snap.yes.mid,
                );
                ui.set_side(
                    &series,
                    lbl,
                    false,
                    snap.no.sequence,
                    snap.no.bids.clone(),
                    snap.no.asks.clone(),
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
    // Silence ALL tracing — the UI uses SAVE/RESTORE cursor positioning and any
    // stray log line would corrupt it.
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

    // Print the tickers we will subscribe to before the UI takes over stdout.
    let now = Utc::now();
    let win_close = current_window_close(now);
    eprintln!(
        "Current 15-min window closes: {} UTC",
        win_close.format("%Y-%m-%d %H:%M")
    );
    for m in &markets {
        eprintln!(
            "  {} → {} (strike resolved at window start)",
            m.series,
            build_event_ticker(&m.series, win_close)
        );
    }

    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(DEPTH)));
    let render_interval = StdDuration::from_millis(RENDER_INTERVAL_MS);

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|m| {
            spawn_scheduler(
                m,
                store.clone(),
                ui.clone(),
                DEPTH,
                EARLY_START_SECS,
                creds.clone(),
            )
        })
        .collect();

    let _render = spawn_render_loop(store, ui, render_interval);

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down.");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticker_uses_eastern_time() {
        // 14:45 UTC = 10:45 EDT (UTC-4 in April) → segment "26APR151045"
        let utc_start = Utc.with_ymd_and_hms(2026, 4, 15, 14, 45, 0).unwrap();
        assert_eq!(format_ticker_dt(utc_start), "26APR151045");
        assert_eq!(
            build_event_ticker("KXBTC15M", utc_start),
            "KXBTC15M-26APR151045"
        );
        assert_eq!(
            build_event_ticker("KXETH15M", utc_start),
            "KXETH15M-26APR151045"
        );
    }

    #[test]
    fn ticker_winter_est_offset() {
        // January: EST = UTC-5
        // 10:30 UTC = 05:30 EST → "26JAN130530"
        let utc_start = Utc.with_ymd_and_hms(2026, 1, 13, 10, 30, 0).unwrap();
        assert_eq!(format_ticker_dt(utc_start), "26JAN130530");
    }

    #[test]
    fn window_boundaries() {
        // :02 → start=:00, close=:15
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 2, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 0);
        assert_eq!(current_window_close(t).minute(), 15);

        // :15 exactly → start=:15, close=:30 (new window just started)
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 15, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 15);
        assert_eq!(current_window_close(t).minute(), 30);

        // :59 → start=:45, close=:00 of next hour
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 59, 0).unwrap();
        assert_eq!(current_window_start(t).minute(), 45);
        let close = current_window_close(t);
        assert_eq!(close.hour(), 9);
        assert_eq!(close.minute(), 0);
    }
}
