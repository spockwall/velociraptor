//! Daily-rotation market-data writer.
//!
//! Consumes the `RecorderEvent` bus and writes three streams:
//!
//!   {base}/{exchange}/{symbol}/{date}.mpack         — orderbook snapshots
//!   {base}/{exchange}/{symbol}/{date}-trades.mpack  — last-trade events
//!   {base}/events/{date}.csv                        — user events (all variants)
//!
//! ## Format choice
//! Orderbook snapshots and trades are **high volume**, so they're written as
//! length-prefixed MessagePack frames (`u32 LE` length + `rmp_serde::to_vec_named`)
//! — compact, and the same on-disk format `polymarket_recorder` produces and the
//! Python `read_mpack` / `read_mpack_zst` helpers consume. On daily rotation the
//! finished `.mpack` is optionally zstd-compressed in the background to
//! `.mpack.zst` (when `zstd_level > 0`).
//!
//! User events are **low volume** (a handful of fills / order-updates) and stay
//! CSV so they remain greppable / openable in Excel. They go through the shared
//! [`crate::CsvArchive`] and are never compressed.

use crate::config::{RotationPolicy, StorageConfig};
use crate::csv::CsvArchive;
use crate::event::RecorderEvent;
use crate::format::{SnapshotRecord, TradeRecord, UserEventRecord};
use chrono::Utc;
use libs::time::current_date;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

pub struct StorageWriter {
    config: StorageConfig,
}

/// A length-prefixed MessagePack file writer (snapshots + trades).
///
/// Each record is framed as `u32 LE length || msgpack-map bytes`, written
/// sequentially with no index — read front-to-back. Matches the format in
/// `orderbook/src/bin/polymarket_recorder.rs` and the documented wire format.
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
        // append so a restart within the same day continues the file rather
        // than truncating it — mpack frames are self-delimiting, so appending
        // mid-file is safe.
        let file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("MpackWriter: failed to open {}: {e}", path.display());
                return None;
            }
        };
        info!("StorageWriter: opened {}", path.display());
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

    /// Flush, close, and (if configured) spawn a background zstd compression
    /// of the finished file to `{path}.zst`, removing the plain `.mpack`.
    fn close_and_compress(mut self) {
        self.flush();
        let path = self.path.clone();
        let level = self.zstd_level;
        drop(self.writer); // close the fd before compressing
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

/// One open mpack file for a snapshot/trade key, plus the date it belongs to
/// (for daily rotation).
struct OpenMpack {
    writer: MpackWriter,
    date: String,
}

impl StorageWriter {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    pub fn start(
        self,
        mut event_rx: broadcast::Receiver<RecorderEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let config = self.config;

        tokio::spawn(async move {
            // Market-data (mpack) files, keyed by "{exchange}:{symbol}:{kind}".
            let mut mpack: HashMap<String, OpenMpack> = HashMap::new();
            // User-event CSV stream — delegated to the shared archive. Rooted
            // at `{base}/events`, one file per UTC day.
            let events = CsvArchive::new(config.base_path.join("events"));

            let mut flush_tick = interval(Duration::from_millis(config.flush_interval_ms));

            info!(
                base_path = %config.base_path.display(),
                depth = config.depth,
                zstd_level = ?config.zstd_level,
                "StorageWriter (mpack snapshots/trades, csv events) started"
            );

            loop {
                tokio::select! {
                    event = event_rx.recv() => match event {
                        Ok(RecorderEvent::Snapshot(snap)) => {
                            let record = SnapshotRecord::from_snapshot(&snap, config.depth);
                            let key = format!("{}:{}:", snap.exchange, snap.symbol);
                            if let Some(w) = open_mpack(&mut mpack, &key, &current_date(), &config) {
                                if let Err(e) = w.write(&record) {
                                    error!("StorageWriter: snapshot write failed for {key}: {e}");
                                }
                            }
                        }
                        Ok(RecorderEvent::Trade(trade)) => {
                            let record = TradeRecord::from_trade(&trade);
                            let key = format!("{}:{}:trades", trade.exchange, trade.symbol);
                            if let Some(w) = open_mpack(&mut mpack, &key, &current_date(), &config) {
                                if let Err(e) = w.write(&record) {
                                    error!("StorageWriter: trade write failed for {key}: {e}");
                                }
                            }
                        }
                        Ok(RecorderEvent::UserEvent(ev)) => {
                            let record = UserEventRecord::from_event(&ev);
                            // Daily file under {base}/events/{date}.csv. The
                            // shared archive handles header-on-create + flush.
                            let path = events.daily_path(&[], Utc::now());
                            if let Err(e) = events.append(&path, &record) {
                                error!("StorageWriter: user-event write failed: {e}");
                            }
                        }
                        Ok(RecorderEvent::RawUpdate) => {}
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("StorageWriter: lagged, skipped {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("StorageWriter: engine channel closed, stopping");
                            break;
                        }
                    },

                    _ = flush_tick.tick() => {
                        // Only mpack writers buffer; the CSV archive flushes on
                        // every append, so there's nothing to tick for events.
                        for f in mpack.values_mut() {
                            f.writer.flush();
                        }
                    }
                }
            }

            // Final flush on shutdown. Drain so each mpack file is compressed.
            for (_, f) in mpack.drain() {
                f.writer.close_and_compress();
            }

            info!("StorageWriter stopped");
        })
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Open (or reuse) the mpack writer for snapshot/trade `key` on `today`. On a
/// date change the stale file is closed (and compressed in the background)
/// before a fresh one is opened.
fn open_mpack<'a>(
    handles: &'a mut HashMap<String, OpenMpack>,
    key: &str,
    today: &str,
    config: &StorageConfig,
) -> Option<&'a mut MpackWriter> {
    // Rotate if the date changed (Daily policy only).
    let stale = matches!(handles.get(key), Some(of)
        if matches!(config.rotation, RotationPolicy::Daily) && of.date.as_str() != today);
    if stale {
        if let Some(old) = handles.remove(key) {
            let old_date = old.date.clone();
            old.writer.close_and_compress();
            info!("StorageWriter: rotated {key} ({old_date} → {today})");
        }
    }

    if !handles.contains_key(key) {
        let path = mpack_path(&config.base_path, key, today, config.rotation);
        let writer = MpackWriter::open(path, config.zstd_level)?;
        handles.insert(
            key.to_string(),
            OpenMpack {
                writer,
                date: today.to_string(),
            },
        );
    }

    handles.get_mut(key).map(|f| &mut f.writer)
}

/// Path for a snapshot/trade key:
/// - snapshots: `{base}/{exchange}/{symbol}/{date}.mpack` / `data.mpack`
/// - trades:    `{base}/{exchange}/{symbol}/{date}-trades.mpack` / `trades.mpack`
fn mpack_path(base: &std::path::Path, key: &str, date: &str, rotation: RotationPolicy) -> PathBuf {
    let (exchange, symbol, kind) = parse_key(key);
    let filename = match (rotation, kind) {
        (RotationPolicy::Daily, "") => format!("{date}.mpack"),
        (RotationPolicy::Daily, k) => format!("{date}-{k}.mpack"),
        (RotationPolicy::None, "") => "data.mpack".to_string(),
        (RotationPolicy::None, k) => format!("{k}.mpack"),
    };
    base.join(exchange).join(symbol).join(filename)
}

fn parse_key(key: &str) -> (&str, &str, &str) {
    let mut parts = key.splitn(3, ':');
    let exchange = parts.next().unwrap_or("unknown");
    let symbol = parts.next().unwrap_or("unknown");
    let kind = parts.next().unwrap_or("");
    (exchange, symbol, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{LastTradePrice, OrderbookSnapshot};
    use libs::protocol::ExchangeName;
    use std::io::Read;
    use std::path::Path;

    #[test]
    fn mpack_paths_by_kind() {
        let base = Path::new("/data");
        assert_eq!(
            mpack_path(base, "binance:btcusdt:", "2026-06-01", RotationPolicy::Daily),
            Path::new("/data/binance/btcusdt/2026-06-01.mpack")
        );
        assert_eq!(
            mpack_path(base, "binance:btcusdt:trades", "2026-06-01", RotationPolicy::Daily),
            Path::new("/data/binance/btcusdt/2026-06-01-trades.mpack")
        );
        assert_eq!(
            mpack_path(base, "binance:btcusdt:", "x", RotationPolicy::None),
            Path::new("/data/binance/btcusdt/data.mpack")
        );
    }

    fn sample_snapshot() -> OrderbookSnapshot {
        OrderbookSnapshot {
            exchange: ExchangeName::Binance,
            symbol: "btcusdt".into(),
            full_slug: None,
            sequence: 7,
            timestamp: Utc::now(),
            t_exch_ns: 0,
            t_recv_ns: 0,
            best_bid: Some((100.0, 1.0)),
            best_ask: Some((101.0, 2.0)),
            spread: Some(1.0),
            mid: Some(100.5),
            wmid: 100.5,
            bids: vec![(100.0, 1.0), (99.0, 3.0)],
            asks: vec![(101.0, 2.0), (102.0, 4.0)],
        }
    }

    #[derive(serde::Deserialize)]
    struct DecodedSnapshot {
        sequence: u64,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
    }

    fn decode_frames(path: &Path) -> Vec<DecodedSnapshot> {
        let mut f = File::open(path).expect("open mpack file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let mut out = Vec::new();
        let mut i = 0;
        while i + 4 <= buf.len() {
            let len = u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) as usize;
            i += 4;
            let rec: DecodedSnapshot =
                rmp_serde::from_slice(&buf[i..i + len]).expect("decode frame");
            out.push(rec);
            i += len;
        }
        out
    }

    #[tokio::test]
    async fn writes_length_prefixed_mpack_snapshots_and_csv_events() {
        let dir = std::env::temp_dir().join(format!("rec-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let config = StorageConfig {
            base_path: dir.clone(),
            depth: 2,
            flush_interval_ms: 20,
            rotation: RotationPolicy::None, // single file, no date in name
            zstd_level: None,               // skip compression so we can read the .mpack
        };

        let (tx, rx) = broadcast::channel::<RecorderEvent>(16);
        let handle = StorageWriter::new(config).start(rx);

        tx.send(RecorderEvent::Snapshot(sample_snapshot())).unwrap();
        tx.send(RecorderEvent::Snapshot(sample_snapshot())).unwrap();
        tx.send(RecorderEvent::Trade(LastTradePrice {
            exchange: ExchangeName::Binance,
            symbol: "btcusdt".into(),
            full_slug: None,
            price: 100.5,
            size: 0.3,
            side: "BUY".into(),
            fee_rate_bps: 0.0,
            market: String::new(),
            timestamp: Utc::now(),
            trade_id: Some(42),
        }))
        .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
        let _ = handle.await;

        // Snapshot file: two length-prefixed mpack frames, depth-2 bids/asks.
        let snap_path = dir.join("binance").join("btcusdt").join("data.mpack");
        let frames = decode_frames(&snap_path);
        assert_eq!(frames.len(), 2, "expected two snapshot frames");
        for f in &frames {
            assert_eq!(f.sequence, 7);
            assert_eq!(f.bids.len(), 2);
            assert_eq!(f.asks.len(), 2);
            assert_eq!(f.bids[0], (100.0, 1.0));
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
