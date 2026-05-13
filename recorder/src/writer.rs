//! Daily-rotation CSV writer.
//!
//! Three streams produced from the `RecorderEvent` bus:
//!
//!   {base}/{exchange}/{symbol}/{date}.csv          — orderbook snapshots
//!   {base}/{exchange}/{symbol}/{date}-trades.csv   — last-trade events
//!   {base}/events/{date}.csv                       — user events (all variants)
//!
//! Each file has a header row first, then one row per event. Format is
//! human-readable: open in Excel, `less`, or `pandas.read_csv` directly.

use crate::config::{RotationPolicy, StorageConfig};
use crate::event::RecorderEvent;
use crate::format::{SnapshotRecord, TradeRecord, UserEventRecord};
use libs::time::current_date;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

pub struct StorageWriter {
    config: StorageConfig,
}

/// What's currently open for one (exchange:symbol:kind or events::) key.
struct OpenFile {
    writer: csv::Writer<std::fs::File>,
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
            let mut handles: HashMap<String, OpenFile> = HashMap::new();
            let mut flush_tick = interval(Duration::from_millis(config.flush_interval_ms));

            info!(
                base_path = %config.base_path.display(),
                depth = config.depth,
                "StorageWriter (CSV) started"
            );

            loop {
                tokio::select! {
                    event = event_rx.recv() => match event {
                        Ok(RecorderEvent::Snapshot(snap)) => {
                            let record = SnapshotRecord::from_snapshot(&snap, config.depth);
                            let key = format!("{}:{}:", snap.exchange, snap.symbol);
                            let today = current_date();
                            let header = SnapshotRecord::header(config.depth);

                            let Some(file) = get_or_open(&mut handles, &key, &today, &config, &header) else {
                                continue;
                            };
                            if let Err(e) = file.writer.write_record(record.row()) {
                                error!("StorageWriter: snapshot write failed for {key}: {e}");
                            }
                        }
                        Ok(RecorderEvent::Trade(trade)) => {
                            let record = TradeRecord::from_trade(&trade);
                            let key = format!("{}:{}:trades", trade.exchange, trade.symbol);
                            let today = current_date();
                            // Header derived from struct field names via `serialize`.
                            let header: Vec<String> = trade_header();

                            let Some(file) = get_or_open(&mut handles, &key, &today, &config, &header) else {
                                continue;
                            };
                            if let Err(e) = file.writer.serialize(&record) {
                                error!("StorageWriter: trade write failed for {key}: {e}");
                            }
                        }
                        Ok(RecorderEvent::UserEvent(ev)) => {
                            let record = UserEventRecord::from_event(&ev);
                            let key = "events::".to_string();
                            let today = current_date();
                            let header: Vec<String> = user_event_header();

                            let Some(file) = get_or_open(&mut handles, &key, &today, &config, &header) else {
                                continue;
                            };
                            if let Err(e) = file.writer.serialize(&record) {
                                error!("StorageWriter: user-event write failed for {key}: {e}");
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
                        for (key, file) in &mut handles {
                            if let Err(e) = file.writer.flush() {
                                error!("StorageWriter: flush failed for {key}: {e}");
                            }
                        }
                    }
                }
            }

            // Final flush on shutdown.
            for (key, file) in &mut handles {
                if let Err(e) = file.writer.flush() {
                    error!("StorageWriter: final flush failed for {key}: {e}");
                }
            }

            info!("StorageWriter stopped");
        })
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Open (or reuse) the CSV writer for `key` on `today`. Writes header
/// when creating a new file; appends when re-opening an existing file
/// (`csv::Writer` does not auto-write headers when wrapping a non-empty
/// file, which is what we want for resume-on-restart).
fn get_or_open<'a>(
    handles: &'a mut HashMap<String, OpenFile>,
    key: &str,
    today: &str,
    config: &StorageConfig,
    header: &[String],
) -> Option<&'a mut OpenFile> {
    // Rotate if date changed.
    let stale = matches!(handles.get(key), Some(of)
        if matches!(config.rotation, RotationPolicy::Daily) && of.date.as_str() != today);
    if stale {
        if let Some(mut old) = handles.remove(key) {
            if let Err(e) = old.writer.flush() {
                error!("StorageWriter: flush on rotate failed for {key}: {e}");
            }
            // old dropped → file closed
            info!("StorageWriter: rotated {key} ({} → {today})", old.date);
        }
    }

    if !handles.contains_key(key) {
        let dir = output_dir(&config.base_path, key);
        if let Err(e) = fs::create_dir_all(&dir) {
            error!("StorageWriter: failed to create dir {}: {e}", dir.display());
            return None;
        }

        let filename = filename_for_key(today, key, config.rotation);
        let path = dir.join(&filename);
        let preexisting = path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false);

        let file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("StorageWriter: failed to open {}: {e}", path.display());
                return None;
            }
        };
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false) // we control header writing ourselves
            .from_writer(file);

        if !preexisting {
            if let Err(e) = writer.write_record(header) {
                error!("StorageWriter: header write failed for {key}: {e}");
                return None;
            }
        }

        handles.insert(
            key.to_string(),
            OpenFile {
                writer,
                date: today.to_string(),
            },
        );
        info!("StorageWriter: opened {}", path.display());
    }

    handles.get_mut(key)
}

/// Output dir for a key.
/// - Market data: `{base}/{exchange}/{symbol}/`
/// - User events: `{base}/events/`
fn output_dir(base: &PathBuf, key: &str) -> PathBuf {
    if key.starts_with("events::") {
        base.join("events")
    } else {
        let (exchange, symbol, _) = parse_key(key);
        base.join(exchange).join(symbol)
    }
}

/// Filename for a key on a given date.
/// - Market-data snapshots: `{date}.csv` / `data.csv`
/// - Market-data trades:    `{date}-trades.csv` / `trades.csv`
/// - User events:           `{date}.csv` / `data.csv`
fn filename_for_key(date: &str, key: &str, rotation: RotationPolicy) -> String {
    if key.starts_with("events::") {
        return match rotation {
            RotationPolicy::Daily => format!("{date}.csv"),
            RotationPolicy::None => "data.csv".to_string(),
        };
    }
    let (_, _, kind) = parse_key(key);
    match (rotation, kind) {
        (RotationPolicy::Daily, "") => format!("{date}.csv"),
        (RotationPolicy::Daily, k) => format!("{date}-{k}.csv"),
        (RotationPolicy::None, "") => "data.csv".to_string(),
        (RotationPolicy::None, k) => format!("{k}.csv"),
    }
}

fn parse_key(key: &str) -> (&str, &str, &str) {
    let mut parts = key.splitn(3, ':');
    let exchange = parts.next().unwrap_or("unknown");
    let symbol = parts.next().unwrap_or("unknown");
    let kind = parts.next().unwrap_or("");
    (exchange, symbol, kind)
}

fn trade_header() -> Vec<String> {
    vec![
        "ts_ns".into(),
        "price".into(),
        "size".into(),
        "side".into(),
        "fee_rate_bps".into(),
        "trade_id".into(),
    ]
}

fn user_event_header() -> Vec<String> {
    // Must match `UserEventRecord` field order, since the CSV writer is
    // header-less and `serialize()` emits fields in declaration order.
    vec![
        "ts_ns".into(),
        "type".into(),
        "exchange".into(),
        "symbol".into(),
        "side".into(),
        "px".into(),
        "qty".into(),
        "filled".into(),
        "status".into(),
        "fee".into(),
        "taker_oid".into(),
        "client_oid".into(),
        "exchange_oid".into(),
        "maker_orders".into(),
    ]
}
