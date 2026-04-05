use crate::config::{RotationPolicy, StorageConfig};
use crate::event::RecorderEvent;
use crate::format::StorageRecord;
use libs::time::current_date;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

pub struct StorageWriter {
    config: StorageConfig,
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
            // key: "exchange:symbol"  →  (writer, current_date_str)
            let mut handles: HashMap<String, (BufWriter<File>, String)> = HashMap::new();
            let mut flush_tick = interval(Duration::from_millis(config.flush_interval_ms));

            info!(
                base_path = %config.base_path.display(),
                depth = config.depth,
                zstd_level = ?config.zstd_level,
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
                        for (key, (writer, _)) in &mut handles {
                            if let Err(e) = writer.flush() {
                                error!("StorageWriter: flush failed for {key}: {e}");
                            }
                        }
                    }
                }
            }

            // Final flush on shutdown
            for (key, (ref mut writer, date)) in &mut handles {
                if let Err(e) = writer.flush() {
                    error!("StorageWriter: final flush failed for {key}: {e}");
                }
                // Compress if daily rotation and zstd enabled
                if matches!(config.rotation, RotationPolicy::Daily) {
                    if let Some(level) = config.zstd_level {
                        let mut parts = key.splitn(2, ':');
                        let exchange = parts.next().unwrap_or("unknown");
                        let symbol = parts.next().unwrap_or("unknown");
                        let path = config
                            .base_path
                            .join(exchange)
                            .join(symbol)
                            .join(format!("{date}.mpack"));
                        spawn_compress(path, level);
                    }
                }
            }

            info!("StorageWriter stopped");
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Spawn a blocking task to compress `path` with zstd, then delete the original.
/// Produces `{path}.zst` alongside the original.
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
            Ok(()) => info!(
                "StorageWriter: compressed {} → {}",
                path.display(),
                zst_path.display()
            ),
            Err(e) => error!(
                "StorageWriter: compression failed for {}: {e}",
                path.display()
            ),
        }
    });
}

fn get_or_open<'a>(
    handles: &'a mut HashMap<String, (BufWriter<File>, String)>,
    key: &str,
    today: &str,
    config: &StorageConfig,
) -> Option<&'a mut BufWriter<File>> {
    let stale = matches!(handles.get(key), Some((_, date))
        if matches!(config.rotation, RotationPolicy::Daily) && date.as_str() != today);

    if stale {
        // Flush and close the old writer before opening a new one.
        if let Some((mut old_writer, old_date)) = handles.remove(key) {
            if let Err(e) = old_writer.flush() {
                error!("StorageWriter: flush on rotate failed for {key}: {e}");
            }
            // old_writer dropped here — file handle closed before compress reads it
            if let Some(level) = config.zstd_level {
                let mut parts = key.splitn(2, ':');
                let exchange = parts.next().unwrap_or("unknown");
                let symbol = parts.next().unwrap_or("unknown");
                let path = config
                    .base_path
                    .join(exchange)
                    .join(symbol)
                    .join(format!("{old_date}.mpack"));
                info!("StorageWriter: rotated {key} ({old_date} → {today})");
                spawn_compress(path, level);
            }
        }
    }

    if !handles.contains_key(key) {
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
    let payload = rmp_serde::to_vec_named(record)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(())
}
