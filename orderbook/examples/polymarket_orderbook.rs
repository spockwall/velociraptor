//! Polymarket orderbook visualizer with window-aware scheduler.
//!
//! The scheduler owns the market lifecycle:
//!   - At startup, resolves the current window's token IDs and starts a MarketTask.
//!   - `EARLY_START_SECS` before each window boundary, starts the next window's task.
//!   - When a window expires, shuts down the old task and the new one takes over.
//!   - The render loop reads from a shared SnapStore and draws at a fixed interval.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example polymarket_orderbook -- --config configs/polymarket.toml
//! cargo run --example polymarket_orderbook -- --slug btc-updown-5m --interval-secs 300
//! ```

use anyhow::Result;
use clap::Parser;
use libs::terminal::PolymarketUi;
use libs::time::now_secs;
use orderbook::configs::exchanges::PolymarketMarket;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::polymarket::{PolymarketSubMsgBuilder, resolve_assets_with_labels};
use orderbook::types::ExchangeName;
use orderbook::{OrderbookEvent, OrderbookSystem, OrderbookSystemConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many seconds before a window ends to start the next window's connection.
const EARLY_START_SECS: u64 = 10;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ExampleConfig {
    depth:           usize,
    render_interval: u64,
    polymarket:      Vec<PolymarketMarket>,
}

impl Default for ExampleConfig {
    fn default() -> Self {
        Self { depth: 8, render_interval: 300, polymarket: vec![] }
    }
}

impl ExampleConfig {
    fn load(path: &str) -> Self {
        let s = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Failed to read config '{path}': {e}");
            std::process::exit(1);
        });
        toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("Failed to parse config '{path}': {e}");
            std::process::exit(1);
        })
    }
}

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[clap(about = "Polymarket live orderbook viewer")]
struct Args {
    #[clap(long)]
    config: Option<String>,

    #[clap(long = "slug", action = clap::ArgAction::Append)]
    slugs: Vec<String>,

    #[clap(long = "interval-secs", action = clap::ArgAction::Append)]
    intervals: Vec<u64>,

    #[clap(long)]
    depth: Option<usize>,

    #[clap(long)]
    render_interval: Option<u64>,
}

// ── Shared store ──────────────────────────────────────────────────────────────

/// Latest orderbook snapshot for one side of a market.
/// Keyed by `"{full_slug}-Up"` or `"{full_slug}-Down"`.
#[derive(Clone)]
struct SideSnap {
    full_slug: String,
    is_up:     bool,
    sequence:  u64,
    bids:      Vec<(f64, f64)>,
    asks:      Vec<(f64, f64)>,
    spread:    Option<f64>,
    mid:       Option<f64>,
}

type SnapStore = Arc<Mutex<HashMap<String, SideSnap>>>;

// ── Market task ───────────────────────────────────────────────────────────────

/// A running orderbook connection for one specific window (identified by `full_slug`).
struct MarketTask {
    full_slug:      String,
    system_control: SystemControl,
    handle:         tokio::task::JoinHandle<()>,
}

impl MarketTask {
    /// Resolve token IDs for `full_slug` (a fully-timestamped slug), start a
    /// connection, and stream snapshots into `store` under `base_slug` keys so
    /// the panel is reused across window rotations.
    async fn spawn(
        market:    &PolymarketMarket,
        base_slug: String,   // panel key, e.g. "btc-updown-5m"
        full_slug: String,   // display title, e.g. "btc-updown-5m-1775320000"
        store:     SnapStore,
        depth:     usize,
    ) -> Option<Self> {
        let single = vec![PolymarketMarket {
            slug:          full_slug.clone(),
            interval_secs: 0, // already fully-resolved slug
            ..market.clone()
        }];

        let labeled = tokio::task::spawn_blocking({
            let single = single.clone();
            move || resolve_assets_with_labels(&single)
        })
        .await
        .ok()?;

        if labeled.is_empty() {
            eprintln!("[scheduler] No tokens for {full_slug} (market not open yet?)");
            return None;
        }

        // token_id → (full_slug, is_up)
        let label_map = Arc::new(Mutex::new(
            labeled.iter()
                .map(|(id, _base, full, is_up)| (id.clone(), (full.clone(), *is_up)))
                .collect::<HashMap<_, _>>()
        ));

        let mut builder = PolymarketSubMsgBuilder::new();
        for (id, _, _, _) in &labeled { builder = builder.with_asset(id); }

        let mut cfg = OrderbookSystemConfig::new();
        cfg.with_exchange(
            ConnectionConfig::new(ExchangeName::Polymarket)
                .set_subscription_message(builder.build()),
        );
        if cfg.validate().is_err() { return None; }

        let control = SystemControl::new();
        let system  = OrderbookSystem::new(cfg, control.clone()).ok()?;

        // Write snapshots into the store keyed by base_slug so the panel persists
        // across window rotations. full_slug is only stored for the display title.
        let store_writer = store.clone();
        let lm           = label_map.clone();
        let base         = base_slug.clone();
        let _event_handle = system.on_update(move |event| {
            let store = store_writer.clone();
            let lm    = lm.clone();
            let base  = base.clone();
            async move {
                if let OrderbookEvent::Snapshot(snap) = event {
                    let (full, is_up) = match lm.lock().ok()
                        .and_then(|m| m.get(&snap.symbol).cloned())
                    {
                        Some(v) => v,
                        None    => return,
                    };
                    // Key by base_slug so old and new windows share the same panel.
                    let key = if is_up { format!("{base}-Up") } else { format!("{base}-Down") };
                    let (bids, asks) = snap.book.depth(depth);
                    if let Ok(mut map) = store.lock() {
                        map.insert(key, SideSnap {
                            full_slug: full, // displayed in the panel title
                            is_up,
                            sequence:  snap.sequence,
                            bids,
                            asks,
                            spread:    snap.book.spread(),
                            mid:       snap.book.mid_price(),
                        });
                    }
                }
            }
        });

        let ctrl = control.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = system.run().await {
                eprintln!("[market-task] system error: {e}");
            }
            ctrl.shutdown();
        });

        eprintln!("[scheduler] Started task for {full_slug}");
        Some(Self { full_slug, system_control: control, handle })
    }

    /// Gracefully stop this task.
    fn stop(self) {
        eprintln!("[scheduler] Stopping task for {}", self.full_slug);
        self.system_control.shutdown();
        self.handle.abort();
    }
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

/// Runs the market lifecycle loop for a single `PolymarketMarket`.
/// Starts the current window, then starts the next window `EARLY_START_SECS`
/// before the boundary and tears down the old one when it expires.
fn spawn_scheduler(
    market: PolymarketMarket,
    store:  SnapStore,
    ui:     Arc<Mutex<PolymarketUi>>,
    depth:  usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let base_slug = market.slug.clone();

        if market.interval_secs == 0 {
            // Static market — start once, panel lives forever under base_slug.
            if let Some(task) = MarketTask::spawn(
                &market, base_slug.clone(), base_slug.clone(), store, depth,
            ).await {
                let _ = task.handle.await;
            }
            return;
        }

        // Ensure the panel exists from the start.
        if let Ok(mut ui) = ui.lock() { ui.ensure(&base_slug); }

        let mut current_task: Option<MarketTask> = None;

        loop {
            let now       = now_secs();
            let interval  = market.interval_secs;
            let win_start = (now / interval) * interval;
            let win_end   = win_start + interval;
            let full_slug = format!("{base_slug}-{win_start}");

            // Start current window task if not already running.
            if current_task.as_ref().map(|t| t.full_slug != full_slug).unwrap_or(true) {
                if let Some(old) = current_task.take() { old.stop(); }
                current_task = MarketTask::spawn(
                    &market, base_slug.clone(), full_slug.clone(), store.clone(), depth,
                ).await;
            }

            // Sleep until EARLY_START_SECS before this window ends.
            let secs_until_early = win_end.saturating_sub(now_secs())
                .saturating_sub(EARLY_START_SECS);
            if secs_until_early > 0 {
                tokio::time::sleep(Duration::from_secs(secs_until_early)).await;
            }

            // Pre-start next window — both tasks write to the same base_slug panel.
            let next_slug = format!("{base_slug}-{win_end}");
            eprintln!("[scheduler] Pre-starting next window: {next_slug}");
            let next_task = MarketTask::spawn(
                &market, base_slug.clone(), next_slug, store.clone(), depth,
            ).await;

            // Wait for current window to fully expire.
            let remaining = win_end.saturating_sub(now_secs());
            if remaining > 0 {
                tokio::time::sleep(Duration::from_secs(remaining)).await;
            }

            // Stop old task; next task is already writing to the panel.
            if let Some(old) = current_task.take() { old.stop(); }
            current_task = next_task;
        }
    })
}

// ── Render loop ───────────────────────────────────────────────────────────────

fn spawn_render_loop(
    store:           SnapStore,
    ui:              Arc<Mutex<PolymarketUi>>,
    render_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(render_interval);
        loop {
            ticker.tick().await;
            let snaps: Vec<(String, SideSnap)> = {
                let Ok(map) = store.lock() else { continue };
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            let Ok(mut ui) = ui.lock() else { continue };
            for (key, s) in snaps {
                // key = "{base_slug}-Up" or "{base_slug}-Down"
                let base_slug = key
                    .strip_suffix("-Up")
                    .or_else(|| key.strip_suffix("-Down"))
                    .unwrap_or(&key);
                ui.update(
                    base_slug, &s.full_slug, s.is_up,
                    s.sequence, s.bids, s.asks, s.spread, s.mid,
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
    let mut cfg = args.config.as_deref().map(ExampleConfig::load).unwrap_or_default();

    if let Some(d) = args.depth           { cfg.depth           = d; }
    if let Some(r) = args.render_interval { cfg.render_interval = r; }

    if !args.slugs.is_empty() {
        cfg.polymarket = args.slugs.into_iter().enumerate()
            .map(|(i, slug)| PolymarketMarket {
                enabled:       true,
                slug,
                interval_secs: *args.intervals.get(i).unwrap_or(&0),
            })
            .collect();
    }

    if cfg.polymarket.is_empty() {
        eprintln!("No markets configured. Pass --config <file> or --slug <slug> --interval-secs <n>.");
        std::process::exit(1);
    }

    let depth           = cfg.depth;
    let render_interval = Duration::from_millis(cfg.render_interval);
    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(depth)));

    // One scheduler per market.
    let _schedulers: Vec<_> = cfg.polymarket.into_iter()
        .map(|market| spawn_scheduler(market, store.clone(), ui.clone(), depth))
        .collect();

    let _render = spawn_render_loop(store, ui, render_interval);

    tokio::signal::ctrl_c().await?;
    Ok(())
}
