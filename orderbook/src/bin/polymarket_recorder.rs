//! Polymarket orderbook recorder.
//!
//! Streams live orderbook data, writes it to disk per-window-per-side, and
//! renders a live terminal UI for monitoring.
//!
//! # Directory layout
//!
//! ```text
//! {base_path}/polymarket/{slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-up.mpack
//!                                            {HH:MM}-{HH:MM}-down.mpack
//! ```
//!
//! Each file contains length-prefixed MessagePack records. Files are optionally
//! zstd-compressed after each window closes, producing `*.mpack.zst`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin polymarket_recorder -- --config configs/polymarket.yaml
//! cargo run --bin polymarket_recorder -- \
//!     --slug btc-updown-5m --interval-secs 300 \
//!     --base-path ./data --depth 10
//! ```

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use libs::configs::{PolymarketFileConfig, PolymarketMarketConfig};
use libs::protocol::ExchangeName;
use libs::terminal::PolymarketUi;
use libs::time::now_secs;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::polymarket::{PolymarketSubMsgBuilder, resolve_assets_with_labels};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use recorder::format::StorageRecord;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

/// How many seconds before a window ends to start the next window's connection.
const EARLY_START_SECS: u64 = 10;

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct Args {
    /// Path to YAML config file (e.g. configs/polymarket.yaml)
    #[clap(long)]
    config: Option<String>,

    /// Market slug (repeatable, e.g. --slug btc-updown-5m)
    #[clap(long = "slug", action = clap::ArgAction::Append)]
    slugs: Vec<String>,

    /// Window size in seconds paired with each --slug (repeatable)
    #[clap(long = "interval-secs", action = clap::ArgAction::Append)]
    intervals: Vec<u64>,

    /// Orderbook depth levels
    #[clap(long)]
    depth: Option<usize>,

    /// Terminal render interval in milliseconds
    #[clap(long)]
    render_interval: Option<u64>,

    /// Root directory for recorded files
    #[clap(long)]
    base_path: Option<String>,

    /// zstd compression level (0 = disabled)
    #[clap(long)]
    zstd_level: Option<u8>,
}

// ── Shared snap store (for UI) ────────────────────────────────────────────────

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

// ── Polymarket storage writer ─────────────────────────────────────────────────

/// Writes orderbook snapshots for one window side (Up or Down) to disk.
///
/// Path: `{base}/{slug}/{date}/{interval_label}-{side}.mpack`
struct PolymarketWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    zstd_level: Option<i32>,
}

impl PolymarketWriter {
    /// Open (or create) the file for this window side.
    fn open(
        base_path: &PathBuf,
        slug: &str, // e.g. "btc-updown-5m"
        win_start_secs: u64,
        win_end_secs: u64,
        is_up: bool,
        zstd_level: Option<i32>,
    ) -> Option<Self> {
        let win_start: DateTime<Utc> = Utc.timestamp_opt(win_start_secs as i64, 0).single()?;
        let win_end: DateTime<Utc> = Utc.timestamp_opt(win_end_secs as i64, 0).single()?;

        let date_str = win_start.format("%Y-%m-%d").to_string();
        let interval_str = format!("{}-{}", win_start.format("%H:%M"), win_end.format("%H:%M"));
        let side_str = if is_up { "up" } else { "down" };
        let filename = format!("{interval_str}-{side_str}.mpack");

        let dir = base_path.join(slug).join(&date_str);
        if let Err(e) = fs::create_dir_all(&dir) {
            error!(
                "PolymarketWriter: failed to create dir {}: {e}",
                dir.display()
            );
            return None;
        }
        let path = dir.join(&filename);

        let file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("PolymarketWriter: failed to open {}: {e}", path.display());
                return None;
            }
        };

        info!("PolymarketWriter: opened {}", path.display());
        Some(Self {
            writer: BufWriter::new(file),
            path,
            zstd_level,
        })
    }

    fn write(&mut self, record: &StorageRecord) -> anyhow::Result<()> {
        let payload = rmp_serde::to_vec_named(record)?;
        let len = payload.len() as u32;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&payload)?;
        Ok(())
    }

    fn flush(&mut self) {
        if let Err(e) = self.writer.flush() {
            error!(
                "PolymarketWriter: flush failed for {}: {e}",
                self.path.display()
            );
        }
    }

    /// Flush, close, and optionally compress. Called when the window expires.
    fn close_and_compress(mut self) {
        self.flush();
        // Drop the BufWriter (closes the file) before compressing.
        let path = self.path.clone();
        let zstd_level = self.zstd_level;
        drop(self.writer);

        if let Some(level) = zstd_level {
            spawn_compress(path, level);
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
            let mut reader = std::io::BufReader::new(input);
            std::io::copy(&mut reader, &mut encoder)?;
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

// ── Market task ───────────────────────────────────────────────────────────────

struct MarketTask {
    full_slug: String,
    system_control: SystemControl,
    handle: tokio::task::JoinHandle<()>,
}

impl MarketTask {
    async fn spawn(
        market: &PolymarketMarketConfig,
        base_slug: String,
        full_slug: String,
        win_start_secs: u64,
        win_end_secs: u64,
        store: SnapStore,
        base_path: PathBuf,
        depth: usize,
        zstd_level: Option<i32>,
    ) -> Option<Self> {
        let single = vec![PolymarketMarketConfig {
            enabled: market.enabled,
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
            warn!("No tokens for {full_slug} (market not open yet?)");
            return None;
        }

        // Build writers for Up and Down sides.
        // Use `base_slug` (from the caller, no timestamp) as the directory name so all
        // windows for the same market land under the same slug folder.
        let mut writers: HashMap<String, PolymarketWriter> = HashMap::new();
        for (_, _, _, is_up) in &labeled {
            let sk = side_key(&full_slug, *is_up);
            if let Some(w) = PolymarketWriter::open(
                &base_path,
                &base_slug,
                win_start_secs,
                win_end_secs,
                *is_up,
                zstd_level,
            ) {
                writers.insert(sk, w);
            }
        }
        let writers = Arc::new(Mutex::new(writers));

        // label_map: token_id → (full_slug, is_up)
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

        let mut cfg = StreamSystemConfig::new();
        cfg.with_exchange(
            ClientConfig::new(ExchangeName::Polymarket).set_subscription_message(builder.build()),
        );
        if cfg.validate().is_err() {
            return None;
        }

        let control = SystemControl::new();
        let mut engine = StreamEngine::new(cfg.event_broadcast_capacity);

        // Flush ticker for writers.
        let writers_flush = writers.clone();
        let flush_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                if let Ok(mut map) = writers_flush.lock() {
                    for w in map.values_mut() {
                        w.flush();
                    }
                }
            }
        });

        let store_writer = store.clone();
        let lm = label_map.clone();
        let base = base_slug.clone();
        let _event_handle = system.on_update(move |event| {
            let store = store_writer.clone();
            let lm = lm.clone();
            let base = base.clone();
            let writers = writers.clone();
            async move {
                if let OrderbookEvent::Snapshot(snap) = event {
                    let (full, is_up) =
                        match lm.lock().ok().and_then(|m| m.get(&snap.symbol).cloned()) {
                            Some(v) => v,
                            None => return,
                        };

                    let sk = side_key(&full, is_up);
                    let book = &snap.book;
                    let (bids, asks) = book.depth(depth);

                    // Write to disk on every update.
                    if let Ok(mut map) = writers.lock() {
                        if let Some(w) = map.get_mut(&sk) {
                            let rec = StorageRecord {
                                sequence: snap.sequence,
                                ts_ns: snap.timestamp.timestamp_nanos_opt().unwrap_or(0),
                                bids: bids.iter().map(|&(p, q)| [p, q]).collect(),
                                asks: asks.iter().map(|&(p, q)| [p, q]).collect(),
                            };
                            if let Err(e) = w.write(&rec) {
                                error!("Write failed for {sk}: {e}");
                            }
                        }
                    }

                    // Update snap store for UI.
                    let store_key = if is_up {
                        format!("{base}-Up")
                    } else {
                        format!("{base}-Down")
                    };
                    if let Ok(mut map) = store.lock() {
                        map.insert(
                            store_key,
                            SideSnap {
                                full_slug: full,
                                is_up,
                                sequence: snap.sequence,
                                bids,
                                asks,
                                spread: book.spread(),
                                mid: book.mid_price(),
                            },
                        );
                    }
                }
            }
        });

        let ctrl = control.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = system.run().await {
                error!("MarketTask system error: {e}");
            }
            ctrl.shutdown();
        });

        info!("Started task for {full_slug}");
        Some(Self {
            full_slug,
            system_control: control,
            handle: tokio::spawn(async move {
                handle.await.ok();
                flush_handle.abort();
            }),
        })
    }

    fn stop(self, writers_to_close: Option<Arc<Mutex<HashMap<String, PolymarketWriter>>>>) {
        info!("Stopping task for {}", self.full_slug);
        self.system_control.shutdown();
        self.handle.abort();
        // Flush and compress completed window files.
        if let Some(writers) = writers_to_close {
            if let Ok(mut map) = writers.lock() {
                for (_, w) in map.drain() {
                    w.close_and_compress();
                }
            }
        }
    }
}

fn side_key(full_slug: &str, is_up: bool) -> String {
    if is_up {
        format!("{full_slug}-Up")
    } else {
        format!("{full_slug}-Down")
    }
}

// ── Scheduler ─────────────────────────────────────────────────────────────────

fn spawn_scheduler(
    market: PolymarketMarketConfig,
    store: SnapStore,
    ui: Arc<Mutex<PolymarketUi>>,
    base_path: PathBuf,
    depth: usize,
    zstd_level: Option<i32>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let base_slug = market.slug.clone();

        if market.interval_secs == 0 {
            if let Ok(mut ui) = ui.lock() {
                ui.ensure(&base_slug);
            }
            if let Some(task) = MarketTask::spawn(
                &market,
                base_slug.clone(),
                base_slug.clone(),
                0,
                0,
                store,
                base_path,
                depth,
                zstd_level,
            )
            .await
            {
                let _ = task.handle.await;
            }
            return;
        }

        if let Ok(mut ui) = ui.lock() {
            ui.ensure(&base_slug);
        }
        let mut current_task: Option<MarketTask> = None;

        loop {
            let now = now_secs();
            let interval = market.interval_secs;
            let win_start = (now / interval) * interval;
            let win_end = win_start + interval;
            let full_slug = format!("{base_slug}-{win_start}");

            if current_task
                .as_ref()
                .map(|t| t.full_slug != full_slug)
                .unwrap_or(true)
            {
                if let Some(old) = current_task.take() {
                    old.stop(None);
                }
                current_task = MarketTask::spawn(
                    &market,
                    base_slug.clone(),
                    full_slug.clone(),
                    win_start,
                    win_end,
                    store.clone(),
                    base_path.clone(),
                    depth,
                    zstd_level,
                )
                .await;
            }

            // Sleep until EARLY_START_SECS before window ends.
            let secs_until_early = win_end
                .saturating_sub(now_secs())
                .saturating_sub(EARLY_START_SECS);
            if secs_until_early > 0 {
                tokio::time::sleep(Duration::from_secs(secs_until_early)).await;
            }

            // Pre-start next window.
            let next_start = win_end;
            let next_end = next_start + interval;
            let next_slug = format!("{base_slug}-{next_start}");
            info!("Pre-starting next window: {next_slug}");
            let next_task = MarketTask::spawn(
                &market,
                base_slug.clone(),
                next_slug,
                next_start,
                next_end,
                store.clone(),
                base_path.clone(),
                depth,
                zstd_level,
            )
            .await;

            // Wait for current window to expire.
            let remaining = win_end.saturating_sub(now_secs());
            if remaining > 0 {
                tokio::time::sleep(Duration::from_secs(remaining)).await;
            }

            // Stop old task and compress its files.
            if let Some(old) = current_task.take() {
                old.stop(None);
            }
            current_task = next_task;
        }
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

    let mut cfg = args
        .config
        .as_deref()
        .map(PolymarketFileConfig::load)
        .unwrap_or_default();

    // CLI overrides.
    if let Some(r) = args.render_interval {
        cfg.server.render_interval = r;
    }
    if let Some(d) = args.depth {
        cfg.storage.depth = d;
    }
    if let Some(p) = args.base_path {
        cfg.storage.base_path = p;
    }
    if let Some(z) = args.zstd_level {
        cfg.storage.zstd_level = z;
    }

    // CLI --slug flags override config markets entirely.
    if !args.slugs.is_empty() {
        cfg.polymarket.markets = args
            .slugs
            .into_iter()
            .enumerate()
            .map(|(i, slug)| PolymarketMarketConfig {
                enabled: true,
                slug,
                interval_secs: *args.intervals.get(i).unwrap_or(&0),
            })
            .collect();
    }

    let markets: Vec<PolymarketMarketConfig> = cfg
        .polymarket
        .markets
        .into_iter()
        .filter(|m| m.enabled)
        .collect();

    if markets.is_empty() {
        eprintln!(
            "No markets configured. Pass --config <file> or --slug <slug> --interval-secs <n>."
        );
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
            spawn_scheduler(
                market,
                store.clone(),
                ui.clone(),
                base_path.clone(),
                depth,
                zstd_level,
            )
        })
        .collect();

    let _render = spawn_render_loop(store, ui, render_interval);

    tokio::signal::ctrl_c().await?;
    Ok(())
}
