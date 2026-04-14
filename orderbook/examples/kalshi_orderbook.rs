//! Kalshi 15-minute BTC/ETH orderbook visualizer with auto-rotating scheduler.
//!
//! Kalshi runs rolling 15-minute binary markets for BTC and ETH price direction.
//! Each market has a ticker like `KXBTC15M-26APR130815-15` where:
//! - `KXBTC15M` = series (15-min BTC)
//! - `26APR130815` = close time in UTC (YYMONDDHHММ)
//! - `-15` = suffix
//!
//! The scheduler computes the current and next ticker deterministically from
//! the wall clock. It connects to both BTC and ETH simultaneously, and
//! rotates each market independently:
//! - 60s before the current window closes, it opens the next window's connection.
//! - When the current window expires, it shuts down the old connection.
//!
//! # Usage
//!
//! ```bash
//! # BTC + ETH (default)
//! cargo run --example kalshi_orderbook
//!
//! # BTC only
//! cargo run --example kalshi_orderbook -- --series KXBTC15M
//!
//! # Multiple series from config file
//! cargo run --example kalshi_orderbook -- --config configs/kalshi.toml
//! ```

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use chrono_tz::US::Eastern;
use clap::Parser;
use libs::configs::load_yaml_or_exit;
use libs::credentials::KalshiCredentials;
use libs::protocol::ExchangeName;
use libs::terminal::PolymarketUi;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::kalshi::KalshiSubMsgBuilder;
use orderbook::{OrderbookEvent, OrderbookSystem, OrderbookSystemConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

// ── Ticker computation ────────────────────────────────────────────────────────

// Kalshi tickers encode the window close time in **US Eastern Time** (EDT/EST,
// DST-aware). A window closing at 2026-04-13T08:15:00Z (UTC) = 04:15 AM EDT
// → segment "26APR130415". We use `chrono-tz` for correct DST handling.

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Convert a UTC close-time to the Kalshi ticker datetime segment.
/// `2026-04-13T08:15:00Z` (= 04:15 EDT) → `"26APR130415"`
fn format_ticker_dt(utc: DateTime<Utc>) -> String {
    let et = utc.with_timezone(&Eastern);
    format!(
        "{:02}{}{:02}{:02}{:02}",
        et.year() % 100,
        MONTHS[(et.month() as usize) - 1],
        et.day(),
        et.hour(),
        et.minute(),
    )
}

/// Compute the UTC close time of the current 15-min window.
/// Windows are aligned on :00, :15, :30, :45 of each UTC hour.
fn current_window_close(now: DateTime<Utc>) -> DateTime<Utc> {
    let min = now.minute();
    let next_boundary = (min / 15 + 1) * 15;
    let delta = next_boundary - min;
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), now.hour(), min, 0)
        .unwrap()
        .checked_add_signed(Duration::minutes(delta as i64))
        .unwrap()
}

/// Build a full market ticker: `"KXBTC15M-26APR130415-15"`.
fn build_ticker(series: &str, close_utc: DateTime<Utc>) -> String {
    format!("{}-{}-15", series, format_ticker_dt(close_utc))
}

// ── Config ────────────────────────────────────────────────────────────────────

/// One series to subscribe (e.g. `KXBTC15M`, `KXETH15M`).
#[derive(Debug, Clone, Deserialize)]
pub struct KalshiSeries {
    /// Kalshi series ticker, e.g. `"KXBTC15M"`.
    pub series: String,
    /// Display label shown in the panel title (e.g. `"BTC 15m"`).
    pub label: Option<String>,
}

impl KalshiSeries {
    fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.series)
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub depth: usize,
    pub render_interval_ms: u64,
    /// How many seconds before close to connect to the next window.
    pub early_start_secs: u64,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            depth: 10,
            render_interval_ms: 300,
            early_start_secs: 60,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct KalshiTomlConfig {
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub markets: Vec<KalshiSeries>,
}

impl KalshiTomlConfig {
    pub fn load(path: &str) -> Self {
        load_yaml_or_exit(path)
    }
}

const DEFAULT_CREDENTIALS_PATH: &str = "credentials/kalshi.toml";

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "kalshi_orderbook",
    about = "Kalshi BTC/ETH 15-min orderbook visualizer with auto-rotating scheduler"
)]
struct Args {
    /// Display + markets config file (default: configs/kalshi.toml)
    #[arg(long)]
    config: Option<String>,
    /// Credentials file (default: credentials/kalshi.toml)
    #[arg(long, default_value = DEFAULT_CREDENTIALS_PATH)]
    credentials: String,
    /// Series ticker to subscribe (repeatable). Default: KXBTC15M + KXETH15M.
    #[arg(long = "series", action = clap::ArgAction::Append)]
    series: Vec<String>,
    /// Orderbook depth per side (default 10)
    #[arg(long)]
    depth: Option<usize>,
    /// Seconds before close to start next window (default 60)
    #[arg(long)]
    early_start_secs: Option<u64>,
}

// ── Shared snapshot store ─────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct MarketSide {
    sequence: u64,
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
    spread: Option<f64>,
    mid: Option<f64>,
}

/// Snapshot for a rolling window — keyed by `series` (e.g. `"KXBTC15M"`).
/// The panel key stays constant across rotations so the UI panel is stable.
#[derive(Clone, Default)]
struct MarketSnap {
    display_label: String, // current full ticker shown in title
    yes: MarketSide,
    no: MarketSide,
}

type SnapStore = Arc<Mutex<HashMap<String, MarketSnap>>>;

// ── Market task (one window connection) ───────────────────────────────────────

struct MarketTask {
    ticker: String,
    series: String,
    system_control: SystemControl,
    handle: tokio::task::JoinHandle<()>,
}

impl MarketTask {
    async fn spawn(
        series: String,
        ticker: String,
        store: SnapStore,
        depth: usize,
        api_key: String,
    ) -> Option<Self> {
        let conn_cfg = ConnectionConfig::new(ExchangeName::Kalshi)
            .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
            .set_api_key(api_key);

        let mut cfg = OrderbookSystemConfig::new();
        cfg.with_exchange(conn_cfg);
        if cfg.validate().is_err() {
            eprintln!("[scheduler:{series}] invalid config for {ticker}");
            return None;
        }

        let control = SystemControl::new();
        let system = OrderbookSystem::new(cfg, control.clone()).ok()?;

        let store_w = store.clone();
        let series_ev = series.clone();
        let ticker_ev = ticker.clone();

        let _event_handle = system.on_update(move |event| {
            let store = store_w.clone();
            let s = series_ev.clone();
            let t = ticker_ev.clone();
            async move {
                let OrderbookEvent::Snapshot(snap) = event else {
                    return;
                };
                let (bids, asks) = snap.book.depth(depth);
                let spread = snap.book.spread();
                let mid = snap.book.mid_price();
                let seq = snap.sequence;
                let mut map = match store.lock() {
                    Ok(m) => m,
                    Err(_) => return,
                };
                let entry = map.entry(s).or_default();
                entry.display_label = t;
                // YES side = bids (rising price contracts)
                entry.yes = MarketSide {
                    sequence: seq,
                    bids,
                    asks: vec![],
                    spread,
                    mid,
                };
                // NO side  = asks (falling price contracts)
                entry.no = MarketSide {
                    sequence: seq,
                    bids: vec![],
                    asks,
                    spread,
                    mid,
                };
            }
        });

        let ctrl = control.clone();
        let series_log = series.clone();
        let ticker_log = ticker.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = system.run().await {
                eprintln!("[market-task:{series_log}] {ticker_log} error: {e}");
            }
            ctrl.shutdown();
        });

        eprintln!("[scheduler:{series}] connected → {ticker}");
        Some(Self {
            ticker,
            series,
            system_control: control,
            handle,
        })
    }

    fn stop(self) {
        eprintln!("[scheduler:{}] dropping {}", self.series, self.ticker);
        self.system_control.shutdown();
        self.handle.abort();
    }
}

// ── Rolling scheduler (one per series) ───────────────────────────────────────

/// Runs the window-rotation loop for one series (e.g. `KXBTC15M`).
///
/// Loop:
/// 1. Compute the close time of the current 15-min window → derive ticker.
/// 2. Connect if not already running.
/// 3. Sleep until `early_start_secs` before the window closes.
/// 4. Pre-start the next window's connection.
/// 5. When the current window expires, drop the old connection.
/// 6. Repeat.
fn spawn_scheduler(
    series: KalshiSeries,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    depth: usize,
    early_start_secs: u64,
    api_key: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let s = series.series.clone();
        let label = series.label().to_string();

        // Pre-create panel so it appears immediately.
        if let Ok(mut ui) = ui.lock() {
            ui.ensure(&s);
        }

        let mut current: Option<MarketTask> = None;

        loop {
            let now = Utc::now();
            let win_close = current_window_close(now);
            let ticker = build_ticker(&s, win_close);
            let win_close_unix = win_close.timestamp() as u64;

            // Connect to this window if we aren't already.
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
                    api_key.clone(),
                )
                .await;
                // Update display label even before data arrives.
                if let Ok(mut map) = store.lock() {
                    let e = map.entry(s.clone()).or_default();
                    e.display_label = format!("{label} {ticker}");
                }
            }

            // Sleep until `early_start_secs` before this window closes.
            let now_unix = Utc::now().timestamp() as u64;
            let secs_until_early = win_close_unix
                .saturating_sub(now_unix)
                .saturating_sub(early_start_secs);

            if secs_until_early > 0 {
                eprintln!("[scheduler:{s}] {ticker} — next rotation check in {secs_until_early}s");
                tokio::time::sleep(StdDuration::from_secs(secs_until_early)).await;
            }

            // Pre-start the next window's connection in parallel.
            let next_close = win_close + Duration::minutes(15);
            let next_ticker = build_ticker(&s, next_close);
            eprintln!("[scheduler:{s}] pre-starting next window → {next_ticker}");
            let next_task = MarketTask::spawn(
                s.clone(),
                next_ticker.clone(),
                store.clone(),
                depth,
                api_key.clone(),
            )
            .await;

            // Wait for the current window to fully expire.
            let now_unix = Utc::now().timestamp() as u64;
            let remaining = win_close_unix.saturating_sub(now_unix);
            if remaining > 0 {
                tokio::time::sleep(StdDuration::from_secs(remaining)).await;
            }

            // Swap: drop current, promote next.
            if let Some(old) = current.take() {
                old.stop();
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
                // YES side (rising / bids)
                ui.update(
                    &series,
                    lbl,
                    true,
                    snap.yes.sequence,
                    snap.yes.bids.clone(),
                    snap.yes.asks.clone(),
                    snap.yes.spread,
                    snap.yes.mid,
                );
                // NO side (falling / asks)
                ui.update(
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
        }
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("off").try_init();

    let args = Args::parse();

    let mut cfg = match &args.config {
        Some(path) => KalshiTomlConfig::load(path),
        None => KalshiTomlConfig::default(),
    };

    let api_key = KalshiCredentials::load(&args.credentials).api_key;

    if let Some(d) = args.depth {
        cfg.display.depth = d;
    }
    if let Some(e) = args.early_start_secs {
        cfg.display.early_start_secs = e;
    }

    // CLI --series flags override config markets entirely.
    if !args.series.is_empty() {
        cfg.markets = args
            .series
            .into_iter()
            .map(|s| KalshiSeries {
                series: s,
                label: None,
            })
            .collect();
    }

    // Default: BTC + ETH 15-min series.
    if cfg.markets.is_empty() {
        cfg.markets = vec![
            KalshiSeries {
                series: "KXBTC15M".into(),
                label: Some("BTC 15m".into()),
            },
            KalshiSeries {
                series: "KXETH15M".into(),
                label: Some("ETH 15m".into()),
            },
        ];
    }

    let depth = cfg.display.depth;
    let early_start_secs = cfg.display.early_start_secs;
    let render_interval = StdDuration::from_millis(cfg.display.render_interval_ms);

    // Confirm computed ticker for the current window.
    let now = Utc::now();
    let win_close = current_window_close(now);
    eprintln!(
        "Current 15-min window closes: {} UTC",
        win_close.format("%Y-%m-%d %H:%M")
    );
    for m in &cfg.markets {
        eprintln!("  {} → {}", m.series, build_ticker(&m.series, win_close));
    }

    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(depth)));

    let _schedulers: Vec<_> = cfg
        .markets
        .into_iter()
        .map(|m| {
            spawn_scheduler(
                m,
                store.clone(),
                ui.clone(),
                depth,
                early_start_secs,
                api_key.clone(),
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
        // API confirmed: ticker KXBTC15M-26APR130415-15 has close_time 2026-04-13T08:15:00Z
        // 08:15 UTC = 04:15 EDT (UTC-4 in April) → segment "26APR130415"
        let utc_close = Utc.with_ymd_and_hms(2026, 4, 13, 8, 15, 0).unwrap();
        assert_eq!(format_ticker_dt(utc_close), "26APR130415");
        assert_eq!(
            build_ticker("KXBTC15M", utc_close),
            "KXBTC15M-26APR130415-15"
        );
        assert_eq!(
            build_ticker("KXETH15M", utc_close),
            "KXETH15M-26APR130415-15"
        );
    }

    #[test]
    fn ticker_winter_est_offset() {
        // January: EST = UTC-5
        // 10:30 UTC = 05:30 EST → "26JAN130530"
        let utc_close = Utc.with_ymd_and_hms(2026, 1, 13, 10, 30, 0).unwrap();
        assert_eq!(format_ticker_dt(utc_close), "26JAN130530");
    }

    #[test]
    fn window_close_boundaries() {
        // :02 → next close is :15
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 2, 0).unwrap();
        assert_eq!(current_window_close(t).minute(), 15);

        // :15 exactly → next close is :30 (new window just started)
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 15, 0).unwrap();
        assert_eq!(current_window_close(t).minute(), 30);

        // :59 → next close is :00 of next hour
        let t = Utc.with_ymd_and_hms(2026, 4, 13, 8, 59, 0).unwrap();
        let close = current_window_close(t);
        assert_eq!(close.hour(), 9);
        assert_eq!(close.minute(), 0);
    }
}
