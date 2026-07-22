//! Kalshi CF Benchmarks value recorder.
//!
//! Subscribes to `BRTI` and `ETHUSD_RTI` by default and records each typed
//! value update as a length-prefixed MessagePack map.
//!
//! # Directory layout
//!
//! ```text
//! {storage.base_path}/cfbenchmarks/{index_id}/{YYYY-MM-DD}.mpack
//! ```
//!
//! With `storage.rotation: "none"`, the filename is `data.mpack`. When daily
//! rotation and zstd are enabled, completed days become `*.mpack.zst`.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin kalshi_cfbenchmark_recorder -- --config configs/dev/kalshi.yaml --kalshi-credentials credentials/dev/kalshi.yaml
//! ```

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use libs::configs::KalshiFileConfig;
use libs::credentials::KalshiCredentials;
use libs::endpoints::kalshi::kalshi;
use libs::logging::init_logging;
use libs::protocol::ExchangeName;
use orderbook::connection::{BaseClientMessage, ClientConfig, ClientTrait, SystemControl};
use orderbook::exchanges::kalshi::{
    KalshiCfBenchmarksClient, KalshiCfBenchmarksMessage, KalshiCfBenchmarksSubMsgBuilder,
    KalshiCfBenchmarksValue,
};
use recorder::RotationPolicy;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

const DEFAULT_CONFIG: &str = "configs/dev/kalshi.yaml";
const DEFAULT_INDICES: [&str; 2] = ["BRTI", "ETHUSD_RTI"];

#[derive(Parser, Debug)]
#[command(about = "Kalshi CF Benchmarks WebSocket recorder")]
struct Args {
    #[arg(long, default_value = DEFAULT_CONFIG)]
    config: String,

    /// Credentials file with a `kalshi:` section (api_key + RSA secret).
    #[arg(
        long,
        env = "KALSHI_CREDENTIALS_FILE",
        default_value = "credentials/dev/kalshi.yaml"
    )]
    kalshi_credentials: String,

    /// CF Benchmarks index ID. Repeat the flag or pass comma-separated IDs.
    /// Defaults to BRTI and ETHUSD_RTI.
    #[arg(long = "index", value_delimiter = ',')]
    indices: Vec<String>,
}

struct MpackWriter {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl MpackWriter {
    fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create directory {}", parent.display()))?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        info!(path = %path.display(), "CF Benchmarks archive opened");
        Ok(Self {
            writer: BufWriter::new(file),
            path,
        })
    }

    fn write<T: serde::Serialize>(&mut self, record: &T) -> Result<()> {
        let payload = rmp_serde::to_vec_named(record)?;
        let len = u32::try_from(payload.len()).context("MessagePack record exceeds 4 GiB")?;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&payload)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.writer
            .flush()
            .with_context(|| format!("flush {}", self.path.display()))
    }

    fn close_and_compress(mut self, zstd_level: Option<i32>) {
        if let Err(err) = self.flush() {
            error!(path = %self.path.display(), "CF Benchmarks final flush failed: {err}");
        }
        let path = self.path.clone();
        drop(self.writer);

        if let Some(level) = zstd_level {
            spawn_compress(path, level);
        }
    }
}

struct OpenWriter {
    date: String,
    writer: MpackWriter,
}

struct BenchmarkArchive {
    base_path: PathBuf,
    rotation: RotationPolicy,
    zstd_level: Option<i32>,
    writers: HashMap<String, OpenWriter>,
}

impl BenchmarkArchive {
    fn new(base_path: PathBuf, rotation: RotationPolicy, zstd_level: Option<i32>) -> Self {
        Self {
            base_path,
            rotation,
            zstd_level,
            writers: HashMap::new(),
        }
    }

    fn write(&mut self, value: &KalshiCfBenchmarksValue) -> Result<()> {
        let date = source_date(value);
        let should_rotate = matches!(self.rotation, RotationPolicy::Daily)
            && self
                .writers
                .get(&value.index_id)
                .is_some_and(|open| open.date != date);

        if should_rotate && let Some(old) = self.writers.remove(&value.index_id) {
            info!(
                index_id = %value.index_id,
                previous_date = %old.date,
                next_date = %date,
                "CF Benchmarks archive rotating"
            );
            old.writer.close_and_compress(self.zstd_level);
        }

        if !self.writers.contains_key(&value.index_id) {
            let path = benchmark_path(&self.base_path, &value.index_id, &date, self.rotation);
            let writer = MpackWriter::open(path)?;
            self.writers
                .insert(value.index_id.clone(), OpenWriter { date, writer });
        }

        self.writers
            .get_mut(&value.index_id)
            .expect("writer inserted above")
            .writer
            .write(value)
    }

    fn flush_all(&mut self) {
        for (index_id, open) in &mut self.writers {
            if let Err(err) = open.writer.flush() {
                error!(%index_id, "CF Benchmarks archive flush failed: {err}");
            }
        }
    }
}

fn benchmark_path(
    base_path: &Path,
    index_id: &str,
    date: &str,
    rotation: RotationPolicy,
) -> PathBuf {
    let filename = match rotation {
        RotationPolicy::Daily => format!("{date}.mpack"),
        RotationPolicy::None => "data.mpack".to_string(),
    };
    base_path.join("cfbenchmarks").join(index_id).join(filename)
}

fn source_date(value: &KalshiCfBenchmarksValue) -> String {
    DateTime::from_timestamp_millis(value.source_data.time)
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string()
}

fn spawn_compress(path: PathBuf, level: i32) {
    tokio::task::spawn_blocking(move || {
        let zst_path = path.with_extension("mpack.zst");
        let result = (|| -> Result<()> {
            let input = File::open(&path)?;
            let output = File::create(&zst_path)?;
            let mut encoder = zstd::Encoder::new(output, level)?;
            std::io::copy(&mut std::io::BufReader::new(input), &mut encoder)?;
            encoder.finish()?;
            fs::remove_file(&path)?;
            Ok(())
        })();

        match result {
            Ok(()) => info!(
                source = %path.display(),
                destination = %zst_path.display(),
                "CF Benchmarks archive compressed"
            ),
            Err(err) => error!(path = %path.display(), "CF Benchmarks compression failed: {err}"),
        }
    });
}

fn normalize_indices(configured: &[String]) -> Result<Vec<String>> {
    let values: Vec<String> = if configured.is_empty() {
        DEFAULT_INDICES
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    } else {
        configured.to_vec()
    };

    let mut seen = HashSet::new();
    let mut indices = Vec::new();
    for value in values {
        let index_id = value.trim();
        if index_id.is_empty()
            || !index_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            bail!("invalid CF Benchmarks index ID: {value:?}");
        }
        if seen.insert(index_id.to_string()) {
            indices.push(index_id.to_string());
        }
    }
    Ok(indices)
}

fn rotation_policy(value: &str) -> Result<RotationPolicy> {
    match value.to_ascii_lowercase().as_str() {
        "daily" => Ok(RotationPolicy::Daily),
        "none" => Ok(RotationPolicy::None),
        other => bail!("unsupported storage.rotation {other:?}; expected 'daily' or 'none'"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = KalshiFileConfig::load(&args.config);
    let _guards = init_logging(
        "cfbenchmark_recorder",
        Path::new(&cfg.logging.dir),
        &cfg.logging.level,
        cfg.logging.json,
    );

    let creds = KalshiCredentials::try_load(&args.kalshi_credentials).with_context(|| {
        format!(
            "Kalshi credentials are required at {}",
            args.kalshi_credentials
        )
    })?;
    let indices = normalize_indices(&args.indices)?;

    let index_refs: Vec<&str> = indices.iter().map(String::as_str).collect();
    let subscription = KalshiCfBenchmarksSubMsgBuilder::new()
        .with_indices(&index_refs)
        .build();
    let client_config = ClientConfig::new(ExchangeName::Kalshi)
        .set_ws_url(kalshi::ws::PUBLIC_STREAM)
        .set_subscription_message(subscription)
        .set_api_credentials(creds.api_key, creds.secret, None);

    let (message_tx, mut message_rx) = mpsc::unbounded_channel();
    let control = SystemControl::new();
    let mut client = KalshiCfBenchmarksClient::new(client_config, message_tx, control.clone())?;
    let mut client_task = tokio::spawn(async move { client.run().await });

    let rotation = rotation_policy(&cfg.storage.rotation)?;
    let zstd_level = (cfg.storage.zstd_level != 0).then_some(cfg.storage.zstd_level as i32);
    let mut archive =
        BenchmarkArchive::new(PathBuf::from(&cfg.storage.base_path), rotation, zstd_level);
    let mut flush_tick =
        tokio::time::interval(Duration::from_millis(cfg.storage.flush_interval.max(1)));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!(
        indices = ?indices,
        base_path = %cfg.storage.base_path,
        "CF Benchmarks recorder started"
    );

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                info!("CF Benchmarks recorder shutdown requested");
                break;
            }
            message = message_rx.recv() => match message {
                Some(KalshiCfBenchmarksMessage::Value(value)) => {
                    if let Err(err) = archive.write(&value) {
                        error!(index_id = %value.index_id, "CF Benchmarks write failed: {err}");
                    }
                }
                Some(KalshiCfBenchmarksMessage::Base(BaseClientMessage::Connected)) => {
                    info!("CF Benchmarks WebSocket connected");
                }
                Some(KalshiCfBenchmarksMessage::Base(BaseClientMessage::Disconnected)) => {
                    warn!("CF Benchmarks WebSocket disconnected");
                }
                Some(KalshiCfBenchmarksMessage::Base(BaseClientMessage::Error(err))) => {
                    error!("CF Benchmarks WebSocket error: {err}");
                }
                Some(KalshiCfBenchmarksMessage::Base(BaseClientMessage::Ping | BaseClientMessage::Pong)) => {
                    debug!("CF Benchmarks WebSocket heartbeat");
                }
                None => {
                    warn!("CF Benchmarks message channel closed");
                    break;
                }
            },
            _ = flush_tick.tick() => archive.flush_all(),
        }
    }

    control.shutdown();
    let client_result = match tokio::time::timeout(Duration::from_secs(2), &mut client_task).await {
        Ok(result) => Some(result),
        Err(_) => {
            client_task.abort();
            let _ = client_task.await;
            None
        }
    };
    archive.flush_all();

    if let Some(result) = client_result {
        result.context("CF Benchmarks client task failed")??;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orderbook::exchanges::kalshi::{KalshiCfBenchmarksAverage, KalshiCfBenchmarksSourceValue};
    use std::io::Read;

    fn sample_value() -> KalshiCfBenchmarksValue {
        KalshiCfBenchmarksValue {
            sid: 1,
            seq: 42,
            index_id: "BRTI".to_string(),
            received_at: 1_700_000_000_123,
            raw_data: r#"{"type":"value","id":"BRTI","time":1700000000123,"value":"68000.12"}"#
                .to_string(),
            source_data: KalshiCfBenchmarksSourceValue {
                value_type: "value".to_string(),
                id: "BRTI".to_string(),
                time: 1_700_000_000_123,
                value: "68000.12".to_string(),
            },
            avg_60s_data: KalshiCfBenchmarksAverage {
                value: "68000.12000000".to_string(),
                window_size: 3,
                window_start_ts_ms: 1_699_999_940_123,
                window_end_ts_exclusive: 1_700_000_000_123,
            },
            last_60s_windowed_average_15min: None,
            recv_timestamp: 1_700_000_000_123_000_000,
        }
    }

    #[test]
    fn defaults_to_requested_indices_and_deduplicates_overrides() {
        assert_eq!(
            normalize_indices(&[]).unwrap(),
            vec!["BRTI".to_string(), "ETHUSD_RTI".to_string()]
        );
        assert_eq!(
            normalize_indices(&["BRTI".into(), "BRTI".into()]).unwrap(),
            vec!["BRTI".to_string()]
        );
        assert!(normalize_indices(&["../BRTI".into()]).is_err());
    }

    #[test]
    fn builds_daily_and_unrotated_paths() {
        let root = Path::new("/data/kalshi");
        assert_eq!(
            benchmark_path(root, "BRTI", "2026-07-22", RotationPolicy::Daily),
            Path::new("/data/kalshi/cfbenchmarks/BRTI/2026-07-22.mpack")
        );
        assert_eq!(
            benchmark_path(root, "ETHUSD_RTI", "ignored", RotationPolicy::None),
            Path::new("/data/kalshi/cfbenchmarks/ETHUSD_RTI/data.mpack")
        );
    }

    #[test]
    fn writes_length_prefixed_typed_value() {
        let temp = tempfile::tempdir().unwrap();
        let value = sample_value();
        let date = source_date(&value);
        let path = benchmark_path(temp.path(), "BRTI", &date, RotationPolicy::Daily);
        let mut archive =
            BenchmarkArchive::new(temp.path().to_path_buf(), RotationPolicy::Daily, None);

        archive.write(&value).unwrap();
        archive.flush_all();
        drop(archive);

        let mut bytes = Vec::new();
        File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let decoded: KalshiCfBenchmarksValue = rmp_serde::from_slice(&bytes[4..4 + len]).unwrap();
        assert_eq!(decoded.index_id, "BRTI");
        assert_eq!(decoded.source_data.value, "68000.12");
        assert!(decoded.last_60s_windowed_average_15min.is_none());
    }
}
