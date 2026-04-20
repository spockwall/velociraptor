//! Polymarket orderbook + last-trade recorder.
//!
//! Streams live orderbook data and last-trade events, writes them to disk
//! per-window-per-side, and renders a live terminal UI for monitoring.
//!
//! # Directory layout
//!
//! ```text
//! {base_path}/polymarket/{slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-up.mpack
//!                                            {HH:MM}-{HH:MM}-down.mpack
//!                                            {HH:MM}-{HH:MM}-trades.mpack
//! ```
//!
//! Each file contains length-prefixed MessagePack records.  Files are
//! optionally zstd-compressed after each window closes, producing `*.mpack.zst`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin polymarket_recorder -- --config configs/polymarket.yaml
//! ```

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use libs::configs::{PolymarketFileConfig, PolymarketMarketConfig};
use libs::protocol::{ExchangeName, LastTradePrice, OrderbookSnapshot};
use libs::terminal::PolymarketUi;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::polymarket::{
    run_rolling_scheduler, resolve_assets_with_labels, PolymarketSubMsgBuilder, WindowTask,
};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use recorder::format::{SnapshotRecord, TradeRecord};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

const DEFAULT_CONFIG: &str = "configs/polymarket.yaml";

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[clap(about = "Polymarket orderbook + trade recorder")]
struct Args {
    #[clap(long, default_value = DEFAULT_CONFIG)]
    config: String,
}

// ── Shared snap store (for terminal UI) ──────────────────────────────────────

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

// ── On-disk writer ────────────────────────────────────────────────────────────

struct MpackWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    zstd_level: Option<i32>,
}

impl MpackWriter {
    fn open(path: PathBuf, zstd_level: Option<i32>) -> Option<Self> {
        if let Some(dir) = path.parent() {
            if let Err(e) = fs::create_dir_all(dir) {
                error!("MpackWriter: failed to create dir {}: {e}", dir.display());
                return None;
            }
        }
        let file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("MpackWriter: failed to open {}: {e}", path.display());
                return None;
            }
        };
        info!("MpackWriter: opened {}", path.display());
        Some(Self {
            writer: BufWriter::new(file),
            path,
            zstd_level,
        })
    }

    fn write<T: serde::Serialize>(&mut self, record: &T) -> anyhow::Result<()> {
        let payload = rmp_serde::to_vec_named(record)?;
        let len = payload.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&payload)?;
        Ok(())
    }

    fn flush(&mut self) {
        if let Err(e) = self.writer.flush() {
            error!("MpackWriter: flush failed for {}: {e}", self.path.display());
        }
    }

    fn close_and_compress(mut self) {
        self.flush();
        let path = self.path.clone();
        let level = self.zstd_level;
        drop(self.writer);
        if let Some(lvl) = level {
            spawn_compress(path, lvl);
        }
    }
}

fn spawn_compress(path: PathBuf, level: i32) {
    tokio::task::spawn_blocking(move || {
        let zst_path = path.with_extension("mpack.zst");
        let result = (|| -> anyhow::Result<()> {
            let input = File::open(&path)?;
            let output = File::create(&zst_path)?;
            let mut encoder = zstd::Encoder::new(output, level)?;
            std::io::copy(&mut std::io::BufReader::new(input), &mut encoder)?;
            encoder.finish()?;
            fs::remove_file(&path)?;
            Ok(())
        })();
        match result {
            Ok(()) => info!("Compressed {} → {}", path.display(), zst_path.display()),
            Err(e) => error!("Compression failed for {}: {e}", path.display()),
        }
    });
}

// ── Window file paths ─────────────────────────────────────────────────────────

fn window_dir(base_path: &PathBuf, slug: &str, win_start_secs: u64) -> Option<(PathBuf, String)> {
    let win_start: DateTime<Utc> = Utc.timestamp_opt(win_start_secs as i64, 0).single()?;
    let date_str = win_start.format("%Y-%m-%d").to_string();
    Some((base_path.join(slug).join(&date_str), date_str))
}

fn snapshot_path(
    base_path: &PathBuf,
    slug: &str,
    win_start_secs: u64,
    win_end_secs: u64,
    is_up: bool,
) -> Option<PathBuf> {
    let win_start: DateTime<Utc> = Utc.timestamp_opt(win_start_secs as i64, 0).single()?;
    let win_end: DateTime<Utc> = Utc.timestamp_opt(win_end_secs as i64, 0).single()?;
    let interval_str = format!("{}-{}", win_start.format("%H:%M"), win_end.format("%H:%M"));
    let side_str = if is_up { "up" } else { "down" };
    let (dir, _) = window_dir(base_path, slug, win_start_secs)?;
    Some(dir.join(format!("{interval_str}-{side_str}.mpack")))
}

fn trade_path(
    base_path: &PathBuf,
    slug: &str,
    win_start_secs: u64,
    win_end_secs: u64,
    is_up: bool,
) -> Option<PathBuf> {
    let win_start: DateTime<Utc> = Utc.timestamp_opt(win_start_secs as i64, 0).single()?;
    let win_end: DateTime<Utc> = Utc.timestamp_opt(win_end_secs as i64, 0).single()?;
    let interval_str = format!("{}-{}", win_start.format("%H:%M"), win_end.format("%H:%M"));
    let side_str = if is_up { "up" } else { "down" };
    let (dir, _) = window_dir(base_path, slug, win_start_secs)?;
    Some(dir.join(format!("{interval_str}-{side_str}-trades.mpack")))
}

// ── Window state (shared between hooks and scheduler) ────────────────────────

struct WindowWriters {
    /// key: "{full_slug}-Up" / "{full_slug}-Down" for snapshots,
    ///      "{full_slug}-trades" for last-trade records.
    writers: HashMap<String, MpackWriter>,
}

impl WindowWriters {
    fn new() -> Self {
        Self {
            writers: HashMap::new(),
        }
    }

    fn insert(&mut self, key: String, w: MpackWriter) {
        self.writers.insert(key, w);
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut MpackWriter> {
        self.writers.get_mut(key)
    }

    fn flush_all(&mut self) {
        for w in self.writers.values_mut() {
            w.flush();
        }
    }

    fn close_all(mut self) {
        for (_, w) in self.writers.drain() {
            w.close_and_compress();
        }
    }
}

fn snap_key(full_slug: &str, is_up: bool) -> String {
    if is_up {
        format!("{full_slug}-Up")
    } else {
        format!("{full_slug}-Down")
    }
}

fn trade_key(full_slug: &str, is_up: bool) -> String {
    if is_up {
        format!("{full_slug}-Up-trades")
    } else {
        format!("{full_slug}-Down-trades")
    }
}

// ── Per-window task spawner ───────────────────────────────────────────────────

struct SpawnArgs {
    base_slug: String,
    base_path: PathBuf,
    depth: usize,
    zstd_level: Option<i32>,
    store: SnapStore,
}

async fn spawn_window(
    args: Arc<SpawnArgs>,
    full_slug: String,
    win_start_secs: u64,
    win_end_secs: u64,
) -> Option<WindowTask> {
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

    // Build the writers map for this window.
    let mut ww = WindowWriters::new();

    for (_, _, _, is_up) in &labeled {
        if let Some(path) = snapshot_path(
            &args.base_path,
            &args.base_slug,
            win_start_secs,
            win_end_secs,
            *is_up,
        ) {
            if let Some(w) = MpackWriter::open(path, args.zstd_level) {
                ww.insert(snap_key(&full_slug, *is_up), w);
            }
        }
    }

    for is_up in [true, false] {
        if let Some(path) = trade_path(
            &args.base_path,
            &args.base_slug,
            win_start_secs,
            win_end_secs,
            is_up,
        ) {
            if let Some(w) = MpackWriter::open(path, args.zstd_level) {
                ww.insert(trade_key(&full_slug, is_up), w);
            }
        }
    }

    let writers = Arc::new(Mutex::new(ww));

    // asset_id → (full_slug, is_up)
    let label_map: HashMap<String, (String, bool)> = labeled
        .iter()
        .map(|(id, _, full, is_up)| (id.clone(), (full.clone(), *is_up)))
        .collect();
    let label_map = Arc::new(label_map);

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
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, args.depth);

    // Periodic flush task.
    let writers_flush = writers.clone();
    let flush_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            if let Ok(mut ww) = writers_flush.lock() {
                ww.flush_all();
            }
        }
    });

    // Snapshot hook.
    {
        let writers = writers.clone();
        let label_map = label_map.clone();
        let store = args.store.clone();
        let base_slug = args.base_slug.clone();
        let full_slug_hook = full_slug.clone();
        let depth = args.depth;
        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
            let (_, is_up) = match label_map.get(&snap.symbol) {
                Some(v) => v,
                None => return,
            };
            let sk = snap_key(&full_slug_hook, *is_up);
            let bids: Vec<(f64, f64)> = snap.bids.iter().take(depth).copied().collect();
            let asks: Vec<(f64, f64)> = snap.asks.iter().take(depth).copied().collect();

            if let Ok(mut ww) = writers.lock() {
                if let Some(w) = ww.get_mut(&sk) {
                    let rec = SnapshotRecord::from_snapshot(snap, depth);
                    if let Err(e) = w.write(&rec) {
                        error!("Snapshot write failed for {sk}: {e}");
                    }
                }
            }

            let store_key = if *is_up {
                format!("{base_slug}-Up")
            } else {
                format!("{base_slug}-Down")
            };
            if let Ok(mut map) = store.lock() {
                map.insert(
                    store_key,
                    SideSnap {
                        full_slug: full_slug_hook.clone(),
                        is_up: *is_up,
                        sequence: snap.sequence,
                        bids,
                        asks,
                        spread: snap.spread,
                        mid: snap.mid,
                    },
                );
            }
        });
    }

    // Last-trade hook — routes to up or down file via label_map.
    {
        let writers = writers.clone();
        let label_map = label_map.clone();
        let full_slug_hook = full_slug.clone();
        engine.hooks_mut().on::<LastTradePrice, _>(move |trade| {
            let is_up = match label_map.get(&trade.symbol) {
                Some((_, is_up)) => *is_up,
                None => return,
            };
            let tk = trade_key(&full_slug_hook, is_up);
            if let Ok(mut ww) = writers.lock() {
                if let Some(w) = ww.get_mut(&tk) {
                    let rec = TradeRecord::from_trade(trade);
                    if let Err(e) = w.write(&rec) {
                        error!("Trade write failed for {tk}: {e}");
                    }
                }
            }
        });
    }

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!("Polymarket recorder system error: {e}");
        }
        ctrl.shutdown();
        flush_handle.abort();
        // Drain and compress all open files.
        if let Ok(ww) = Arc::try_unwrap(writers).map(|m| m.into_inner()) {
            if let Ok(ww) = ww {
                ww.close_all();
            }
        }
    });

    info!(full_slug = %full_slug, "Window started");
    Some(WindowTask::new(full_slug, control, handle))
}

// ── Per-market scheduler ──────────────────────────────────────────────────────

fn spawn_market_scheduler(
    market: PolymarketMarketConfig,
    base_path: PathBuf,
    depth: usize,
    zstd_level: Option<i32>,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
) -> tokio::task::JoinHandle<()> {
    let base_slug = market.slug.clone();
    let interval_secs = market.interval_secs;

    if let Ok(mut ui) = ui.lock() {
        ui.ensure(&base_slug);
    }

    let args = Arc::new(SpawnArgs {
        base_slug: base_slug.clone(),
        base_path,
        depth,
        zstd_level,
        store,
    });

    tokio::spawn(async move {
        run_rolling_scheduler(base_slug, interval_secs, move |full_slug| {
            let args = args.clone();
            async move {
                // For static markets (interval_secs == 0), win_start/end are 0.
                // For windowed markets the scheduler itself controls timing;
                // we derive the current window bounds here.
                let (win_start, win_end) = if interval_secs == 0 {
                    (0u64, 0u64)
                } else {
                    let now = libs::time::now_secs();
                    let ws = (now / interval_secs) * interval_secs;
                    (ws, ws + interval_secs)
                };
                spawn_window(args, full_slug, win_start, win_end).await
            }
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

    let args = Args::parse();
    let cfg = PolymarketFileConfig::load(&args.config);

    let markets: Vec<PolymarketMarketConfig> = cfg
        .polymarket
        .markets
        .into_iter()
        .filter(|m| m.enabled)
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {}.", args.config);
        std::process::exit(1);
    }

    let depth = cfg.storage.depth;
    let render_interval = Duration::from_millis(cfg.server.render_interval);
    let base_path = PathBuf::from(&cfg.storage.base_path);
    let zstd_level = match cfg.storage.zstd_level {
        0 => None,
        l => Some(l as i32),
    };
    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));
    let ui = Arc::new(Mutex::new(PolymarketUi::new(depth)));

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|market| {
            spawn_market_scheduler(
                market,
                base_path.clone(),
                depth,
                zstd_level,
                store.clone(),
                ui.clone(),
            )
        })
        .collect();

    let _render = spawn_render_loop(store, ui, render_interval);

    tokio::signal::ctrl_c().await?;
    Ok(())
}
