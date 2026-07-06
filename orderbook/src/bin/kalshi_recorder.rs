//! Kalshi orderbook recorder.
//!
//! Streams live orderbook data for rolling 15-minute Kalshi markets and
//! writes snapshots to disk per window. Unlike Polymarket there is no
//! up/down split — each window has a single ticker whose YES/NO book is
//! folded into one two-sided book by the parser — and the Kalshi feed
//! carries no trade events, so only snapshots are recorded.
//!
//! # Directory layout
//!
//! ```text
//! {base_path}/{series}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}.mpack
//! ```
//!
//! Each file contains length-prefixed MessagePack records.  Files are
//! optionally zstd-compressed after each window closes, producing `*.mpack.zst`.
//!
//! # Usage
//!
//! Kalshi WS market data requires RSA-PSS auth on the upgrade, so
//! credentials are mandatory (unlike the Polymarket recorder).
//!
//! ```bash
//! cargo run --bin kalshi_recorder -- --config configs/dev/kalshi.yaml \
//!     --kalshi-credentials credentials/dev/kalshi.yaml
//! ```

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use libs::configs::{KalshiFileConfig, KalshiMarketConfig};
use libs::credentials::KalshiCredentials;
use libs::endpoints::kalshi::kalshi;
use libs::logging::init_logging;
use libs::protocol::{ExchangeName, OrderbookSnapshot};
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::exchanges::kalshi::{run_rolling_scheduler, KalshiSubMsgBuilder, WindowTask};
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use recorder::format::SnapshotRecord;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info};

const DEFAULT_CONFIG: &str = "configs/dev/kalshi.yaml";

/// Kalshi 15-min windows are aligned on UTC :00/:15/:30/:45.
const KALSHI_INTERVAL_SECS: u64 = 900;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[clap(about = "Kalshi orderbook recorder")]
struct Args {
    #[clap(long, default_value = DEFAULT_CONFIG)]
    config: String,

    /// Credentials file with a `kalshi:` section (api_key + RSA secret).
    /// Required — Kalshi signs the WS upgrade even for market data.
    #[clap(
        long,
        env = "KALSHI_CREDENTIALS_FILE",
        default_value = "credentials/dev/kalshi.yaml"
    )]
    kalshi_credentials: String,
}

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

// ── Window file path ──────────────────────────────────────────────────────────

fn snapshot_path(
    base_path: &PathBuf,
    series: &str,
    win_start_secs: u64,
    win_end_secs: u64,
) -> Option<PathBuf> {
    let win_start: DateTime<Utc> = Utc.timestamp_opt(win_start_secs as i64, 0).single()?;
    let win_end: DateTime<Utc> = Utc.timestamp_opt(win_end_secs as i64, 0).single()?;
    let date_str = win_start.format("%Y-%m-%d").to_string();
    let interval_str = format!("{}-{}", win_start.format("%H:%M"), win_end.format("%H:%M"));
    Some(
        base_path
            .join(series)
            .join(date_str)
            .join(format!("{interval_str}.mpack")),
    )
}

// ── Per-window task spawner ───────────────────────────────────────────────────

struct SpawnArgs {
    series: String,
    base_path: PathBuf,
    depth: usize,
    zstd_level: Option<i32>,
    creds: KalshiCredentials,
}

type SharedWriter = Arc<Mutex<Option<MpackWriter>>>;

async fn spawn_window(
    args: Arc<SpawnArgs>,
    ticker: String,
    win_start_secs: u64,
    win_end_secs: u64,
) -> Option<WindowTask> {
    let path = snapshot_path(&args.base_path, &args.series, win_start_secs, win_end_secs)?;
    let writer: SharedWriter = Arc::new(Mutex::new(Some(MpackWriter::open(
        path,
        args.zstd_level,
    )?)));

    let conn_cfg = ClientConfig::new(ExchangeName::Kalshi)
        .set_ws_url(kalshi::ws::PUBLIC_STREAM)
        .set_subscription_message(KalshiSubMsgBuilder::new().with_ticker(&ticker).build())
        .set_api_credentials(args.creds.api_key.clone(), args.creds.secret.clone(), None);

    let mut cfg = StreamSystemConfig::new();
    cfg.with_exchange(conn_cfg);
    cfg.set_snapshot_depth(args.depth);
    cfg.validate().ok()?;

    let control = SystemControl::new();
    let mut engine = StreamEngine::new(cfg.event_broadcast_capacity, args.depth);

    // Periodic flush task.
    let writer_flush = writer.clone();
    let flush_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            if let Ok(mut guard) = writer_flush.lock() {
                match guard.as_mut() {
                    Some(w) => w.flush(),
                    None => break,
                }
            }
        }
    });

    // Snapshot hook — single book per window, no side routing needed.
    {
        let writer = writer.clone();
        let depth = args.depth;
        let ticker_hook = ticker.clone();
        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
            if let Ok(mut guard) = writer.lock() {
                if let Some(w) = guard.as_mut() {
                    let rec = SnapshotRecord::from_snapshot(snap, depth);
                    if let Err(e) = w.write(&rec) {
                        error!("Snapshot write failed for {ticker_hook}: {e}");
                    }
                }
            }
        });
    }

    let system = StreamSystem::new(engine, cfg, control.clone()).ok()?;
    let ctrl = control.clone();
    let ticker_log = ticker.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = system.run().await {
            error!(ticker = %ticker_log, "Kalshi recorder system error: {e}");
        }
        ctrl.shutdown();
        let writer = writer.lock().ok().and_then(|mut guard| guard.take());
        flush_handle.abort();
        let _ = flush_handle.await;
        if let Some(w) = writer {
            w.close_and_compress();
        }
    });

    info!(ticker = %ticker, "Window started");
    Some(WindowTask::new(ticker, control, handle))
}

// ── Per-series scheduler ──────────────────────────────────────────────────────

fn spawn_series_scheduler(
    market: KalshiMarketConfig,
    base_path: PathBuf,
    depth: usize,
    zstd_level: Option<i32>,
    creds: KalshiCredentials,
) -> tokio::task::JoinHandle<()> {
    let series = market.series.clone();

    let args = Arc::new(SpawnArgs {
        series: series.clone(),
        base_path,
        depth,
        zstd_level,
        creds,
    });

    tokio::spawn(async move {
        run_rolling_scheduler(series, move |ticker| {
            let args = args.clone();
            async move {
                // The scheduler computes the ticker locally; derive the file
                // interval by snapping `now + 30s` down to the 15-min boundary.
                // At initial start `now` is mid-window; at rollover it is just
                // past the boundary — both land inside the new window.
                let win_start =
                    (libs::time::now_secs() + 30) / KALSHI_INTERVAL_SECS * KALSHI_INTERVAL_SECS;
                let win_end = win_start + KALSHI_INTERVAL_SECS;
                spawn_window(args, ticker, win_start, win_end).await
            }
        })
        .await;
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load config first so the `logging:` section can drive tracing setup.
    let cfg = KalshiFileConfig::load(&args.config);
    let _guards = init_logging(
        "kalshi_recorder",
        std::path::Path::new(&cfg.logging.dir),
        &cfg.logging.level,
        cfg.logging.json,
    );

    // Kalshi signs the WS upgrade even for market data — no creds, no stream.
    let creds = match KalshiCredentials::try_load(&args.kalshi_credentials) {
        Some(c) => c,
        None => {
            eprintln!(
                "No kalshi credentials at {} — required for the Kalshi WS market stream.",
                args.kalshi_credentials
            );
            std::process::exit(1);
        }
    };

    let markets: Vec<KalshiMarketConfig> = cfg
        .kalshi
        .market
        .into_iter()
        .filter(|m| m.enable)
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {}.", args.config);
        std::process::exit(1);
    }

    let depth = cfg.storage.depth;
    let base_path = PathBuf::from(&cfg.storage.base_path);
    let zstd_level = match cfg.storage.zstd_level {
        0 => None,
        l => Some(l as i32),
    };

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|market| {
            spawn_series_scheduler(market, base_path.clone(), depth, zstd_level, creds.clone())
        })
        .collect();

    tokio::signal::ctrl_c().await?;
    Ok(())
}
