use crate::config::{RotationPolicy, StorageConfig};
use crate::format::StorageRecord;
use chrono::Utc;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};
use tracing::{error, info, warn};

// Import only the event types — defined as a local mirror so recorder has no
// dependency on the orderbook crate.  The caller passes a typed Receiver.
use crate::event::RecorderEvent;

pub struct StorageWriter {
    config: StorageConfig,
}

impl StorageWriter {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    /// Spawn the writer task.
    ///
    /// Pass the receiver from `engine.subscribe()` directly — recorder does not
    /// depend on `OrderbookEngine` or the `orderbook` crate.
    ///
    /// ```rust
    /// let rx = system.engine().subscribe();
    /// // convert OrderbookEvent → RecorderEvent using the From impl in recorder::event
    /// let handle = StorageWriter::new(config).start(rx);
    /// system.attach_handle(handle);
    /// ```
    pub fn start(
        self,
        mut event_rx: broadcast::Receiver<RecorderEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let config = self.config;

        tokio::spawn(async move {
            // key: "exchange:symbol"  →  (writer, current_date_str)
            let mut handles: HashMap<String, (BufWriter<File>, String)> = HashMap::new();
            let mut flush_tick = interval(Duration::from_millis(config.flush_interval_ms));

            info!(
                base_path = %config.base_path.display(),
                depth = config.depth,
                "StorageWriter started"
            );

            loop {
                tokio::select! {
                    event = event_rx.recv() => match event {
                        Ok(RecorderEvent::Snapshot(snap)) => {
                            let record = StorageRecord::from_snapshot(&snap, config.depth);
                            let key = format!("{}:{}", snap.exchange, snap.symbol);
                            let today = current_date();

                            let writer = match get_or_open(&mut handles, &key, &today, &config) {
                                Some(w) => w,
                                None => continue,
                            };

                            if let Err(e) = write_record(writer, &record) {
                                error!("StorageWriter: write failed for {key}: {e}");
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
                        let today = current_date();
                        handles.retain(|key, (writer, date)| {
                            let stale = matches!(config.rotation, RotationPolicy::Daily)
                                && date.as_str() != today;
                            if stale {
                                let _ = writer.flush();
                                info!("StorageWriter: rotated {key} ({date} → {today})");
                                return false;
                            }
                            if let Err(e) = writer.flush() {
                                error!("StorageWriter: flush failed for {key}: {e}");
                            }
                            true
                        });
                    }
                }
            }

            for (key, (mut writer, _)) in handles {
                if let Err(e) = writer.flush() {
                    error!("StorageWriter: final flush failed for {key}: {e}");
                }
            }
            info!("StorageWriter stopped");
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn current_date() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

fn get_or_open<'a>(
    handles: &'a mut HashMap<String, (BufWriter<File>, String)>,
    key: &str,
    today: &str,
    config: &StorageConfig,
) -> Option<&'a mut BufWriter<File>> {
    let needs_open = match handles.get(key) {
        None => true,
        Some((_, date)) => {
            matches!(config.rotation, RotationPolicy::Daily) && date.as_str() != today
        }
    };

    if needs_open {
        let mut parts = key.splitn(2, ':');
        let exchange = parts.next().unwrap_or("unknown");
        let symbol = parts.next().unwrap_or("unknown");

        let dir = config.base_path.join(exchange).join(symbol);
        if let Err(e) = fs::create_dir_all(&dir) {
            error!("StorageWriter: failed to create dir {}: {e}", dir.display());
            return None;
        }

        let filename = match config.rotation {
            RotationPolicy::Daily => format!("{today}.mpack"),
            RotationPolicy::None => "data.mpack".to_string(),
        };
        let path = dir.join(&filename);

        let file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("StorageWriter: failed to open {}: {e}", path.display());
                return None;
            }
        };

        handles.insert(key.to_string(), (BufWriter::new(file), today.to_string()));
        info!("StorageWriter: opened {}", path.display());
    }

    handles.get_mut(key).map(|(w, _)| w)
}

fn write_record(writer: &mut BufWriter<File>, record: &StorageRecord) -> anyhow::Result<()> {
    let payload = rmp_serde::to_vec(record)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(())
}
