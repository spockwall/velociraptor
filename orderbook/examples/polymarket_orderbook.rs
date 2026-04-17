//! Polymarket orderbook visualizer with window-aware scheduler.
//!
//! The scheduler owns the market lifecycle:
//!   - At startup, resolves the current window's token IDs and starts a MarketTask.
//!   - `EARLY_START_SECS` before each window boundary, starts the next window's task.
//!   - When a window expires, shuts down the old task and the new one takes over.
//!   - The render loop reads from a shared SnapStore and draws at a fixed interval.
//!
//! Config is read from `configs/polymarket.yaml`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example polymarket_orderbook
//! ```

use anyhow::Result;
use libs::configs::{PolymarketFileConfig, PolymarketMarketConfig};
use libs::protocol::ExchangeName;
use libs::terminal::PolymarketUi;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::polymarket::{
    PolymarketSubMsgBuilder, WindowTask, resolve_assets_with_labels, run_rolling_scheduler,
};
use orderbook::{OrderbookEvent, OrderbookSystem, OrderbookSystemConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CONFIG_PATH: &str = "configs/polymarket.yaml";

// ── Shared store ──────────────────────────────────────────────────────────────

/// Latest orderbook snapshot for one side of a market.
/// Keyed by `"{full_slug}-Up"` or `"{full_slug}-Down"`.
#[derive(Clone)]
struct SideSnap {
    full_slug: String,
    is_up: bool,
    sequence: u64,
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
    spread: Option<f64>,
    mid: Option<f64>,
}

type SnapStore = Arc<Mutex<HashMap<String, SideSnap>>>;

// ── Per-window market task ────────────────────────────────────────────────────

/// Spawn a public-market orderbook task for one specific window (`full_slug`).
/// Returns a `WindowTask` that the shared scheduler will manage.
async fn spawn_market_task(
    market: &PolymarketMarketConfig,
    base_slug: String,
    full_slug: String,
    store: SnapStore,
    depth: usize,
) -> Option<WindowTask> {
    let single = vec![PolymarketMarketConfig {
        slug: full_slug.clone(),
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
        labeled
            .iter()
            .map(|(id, _base, full, is_up)| (id.clone(), (full.clone(), *is_up)))
            .collect::<HashMap<_, _>>(),
    ));

    let mut builder = PolymarketSubMsgBuilder::new();
    for (id, _, _, _) in &labeled {
        builder = builder.with_asset(id);
    }

    let mut cfg = OrderbookSystemConfig::new();
    cfg.with_exchange(
        ConnectionConfig::new(ExchangeName::Polymarket).set_subscription_message(builder.build()),
    );
    if cfg.validate().is_err() {
        return None;
    }

    let control = SystemControl::new();
    let system = OrderbookSystem::new(cfg, control.clone()).ok()?;

    // Write snapshots into the store keyed by base_slug so the panel persists
    // across window rotations. full_slug is only stored for the display title.
    let store_writer = store.clone();
    let lm = label_map.clone();
    let base = base_slug.clone();
    let _event_handle = system.on_update(move |event| {
        let store = store_writer.clone();
        let lm = lm.clone();
        let base = base.clone();
        async move {
            if let OrderbookEvent::Snapshot(snap) = event {
                let (full, is_up) = match lm.lock().ok().and_then(|m| m.get(&snap.symbol).cloned())
                {
                    Some(v) => v,
                    None => return,
                };
                let key = if is_up {
                    format!("{base}-Up")
                } else {
                    format!("{base}-Down")
                };
                let (bids, asks) = snap.book.depth(depth);
                if let Ok(mut map) = store.lock() {
                    map.insert(
                        key,
                        SideSnap {
                            full_slug: full,
                            is_up,
                            sequence: snap.sequence,
                            bids,
                            asks,
                            spread: snap.book.spread(),
                            mid: snap.book.mid_price(),
                        },
                    );
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
    Some(WindowTask::new(full_slug, control, handle))
}

// ── Scheduler (thin wrapper over the shared rolling-window loop) ──────────────

fn spawn_scheduler(
    market: PolymarketMarketConfig,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    depth: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let base_slug = market.slug.clone();

        // Ensure the panel exists from the start.
        if let Ok(mut ui) = ui.lock() {
            ui.ensure(&base_slug);
        }

        run_rolling_scheduler(base_slug.clone(), market.interval_secs, move |full_slug| {
            let market = market.clone();
            let base_slug = base_slug.clone();
            let store = store.clone();
            async move { spawn_market_task(&market, base_slug, full_slug, store, depth).await }
        })
        .await;
    })
}

// ── Render loop ───────────────────────────────────────────────────────────────

fn spawn_render_loop(
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
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
                let base_slug = key
                    .strip_suffix("-Up")
                    .or_else(|| key.strip_suffix("-Down"))
                    .unwrap_or(&key);
                ui.update(
                    base_slug,
                    &s.full_slug,
                    s.is_up,
                    s.sequence,
                    s.bids,
                    s.asks,
                    s.spread,
                    s.mid,
                );
            }
        }
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("off").try_init();

    let cfg = PolymarketFileConfig::load(CONFIG_PATH);

    let markets: Vec<PolymarketMarketConfig> = cfg
        .polymarket
        .markets
        .iter()
        .filter(|m| m.enabled)
        .cloned()
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {CONFIG_PATH}");
        std::process::exit(1);
    }

    let depth = cfg.storage.depth;
    let render_interval = Duration::from_millis(cfg.server.render_interval);
    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(depth)));

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|market| spawn_scheduler(market, store.clone(), ui.clone(), depth))
        .collect();

    let _render = spawn_render_loop(store, ui, render_interval);

    tokio::signal::ctrl_c().await?;
    Ok(())
}
