//! Dual-sink audit log: redis stream + on-disk CSV file.
//!
//! Each row carries:
//!   - `seq`         — monotonic counter (per executor process).
//!   - `prev_hash`   — BLAKE3 of the previous row's encoded bytes (32 bytes,
//!                     hex-encoded), so tampering can be detected later.
//!   - `ts_iso`      — RFC3339 timestamp.
//!   - `ts_ns`       — same instant in nanos since UNIX epoch.
//!   - `kind`        — `request` | `response` | `synthetic`.
//!   - `req_id`      — for request/response rows; empty for synthetic.
//!   - `op`          — request action / response result kind / synthetic op.
//!   - `exchange`    — request exchange; empty for response/synthetic.
//!   - `payload_json`— JSON-encoded full payload (the variable bits).
//!
//! Files are `dir/{YYYY-MM-DD}.csv`. The Redis stream sink still carries the
//! mpack bytes for backwards compatibility with downstream consumers; the
//! file sink is the human-readable copy.

use std::path::PathBuf;

use chrono::Utc;
use libs::protocol::orders::{OrderError, OrderRequest, OrderResult};
use libs::redis_client::keys::Executor;
use libs::redis_client::RedisHandle;
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::utils::audit_csv;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Request {
        req: OrderRequest,
    },
    Response {
        req_id: u64,
        result: Result<OrderResult, OrderError>,
    },
    Synthetic {
        op: String,
        detail: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub prev_hash: String,
    pub ts_ns: i64,
    pub payload: Payload,
}

struct State {
    seq: u64,
    /// BLAKE3 hex of the most recent encoded entry; "0"*64 at startup.
    prev_hash: String,
    file: Option<tokio::fs::File>,
    dirty: bool,
}

pub struct AuditSink {
    redis: Option<RedisHandle>,
    stream_cap: usize,
    inner: tokio::sync::Mutex<State>,
}

impl AuditSink {
    /// Open `dir/{date}.csv` and connect to the redis stream. Either sink
    /// may be absent (passing `redis: None` skips the stream sink).
    pub async fn open(
        dir: PathBuf,
        redis: Option<RedisHandle>,
        stream_cap: usize,
    ) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&dir).await?;
        let date = Utc::now().format("%Y-%m-%d");
        let path = dir.join(format!("{date}.csv"));
        let existed = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        if !existed {
            file.write_all(audit_csv::CSV_HEADER).await?;
        }
        Ok(Self {
            redis,
            stream_cap,
            inner: tokio::sync::Mutex::new(State {
                seq: 0,
                prev_hash: "0".repeat(64),
                file: Some(file),
                dirty: false,
            }),
        })
    }

    /// Append a `Request` entry.
    pub async fn log_request(&self, req: &OrderRequest) {
        self.append(Payload::Request { req: req.clone() }, false).await;
    }

    /// Append a `Response` entry. `critical` triggers fsync.
    pub async fn log_response(
        &self,
        req_id: u64,
        result: &Result<OrderResult, OrderError>,
        critical: bool,
    ) {
        self.append(
            Payload::Response {
                req_id,
                result: result.clone(),
            },
            critical,
        )
        .await;
    }

    /// Append a `Synthetic` event. Always fsynced.
    pub async fn log_synthetic(&self, op: impl Into<String>, detail: serde_json::Value) {
        self.append(
            Payload::Synthetic {
                op: op.into(),
                detail,
            },
            true,
        )
        .await;
    }

    async fn append(&self, payload: Payload, critical: bool) {
        let mut g = self.inner.lock().await;
        let now = Utc::now();
        let ts_ns = now.timestamp_nanos_opt().unwrap_or(0);
        let entry = AuditEntry {
            seq: g.seq,
            prev_hash: g.prev_hash.clone(),
            ts_ns,
            payload: payload.clone(),
        };
        // mpack bytes still drive the Redis stream + the hash chain so the
        // chain semantics are unchanged.
        let mpack = match rmp_serde::to_vec_named(&entry) {
            Ok(b) => b,
            Err(e) => {
                warn!("audit: mpack encode failed: {e}");
                return;
            }
        };
        let next_hash = blake3::hash(&mpack).to_hex().to_string();

        // Render a CSV row (one line; payload JSON quoted by csv::Writer).
        let csv_line = match audit_csv::render_row(&entry, &now.to_rfc3339()) {
            Ok(s) => s,
            Err(e) => {
                warn!("audit: csv encode failed: {e}");
                return;
            }
        };

        // Update chain state.
        g.seq += 1;
        g.prev_hash = next_hash;

        // Sink 1: redis stream (unchanged — mpack bytes).
        if let Some(redis) = &self.redis {
            let mut conn = redis.raw();
            let cap = self.stream_cap.to_string();
            let res = redis::cmd("XADD")
                .arg(Executor::LOG_STREAM)
                .arg("MAXLEN")
                .arg("~")
                .arg(&cap)
                .arg("*")
                .arg("payload")
                .arg(mpack.as_slice())
                .query_async::<String>(&mut conn)
                .await;
            if let Err(e) = res {
                warn!("audit: redis XADD failed: {e}");
            }
        }

        // Sink 2: append-only CSV file.
        if let Some(file) = g.file.as_mut() {
            if let Err(e) = file.write_all(csv_line.as_bytes()).await {
                warn!("audit: file write failed: {e}");
            } else if critical {
                if let Err(e) = file.sync_data().await {
                    warn!("audit: fsync failed: {e}");
                }
                g.dirty = false;
            } else {
                g.dirty = true;
            }
        }
    }

    /// Read today's audit CSV. Best-effort: a malformed row ends the scan;
    /// earlier rows are returned. Returns an empty Vec if the file is
    /// missing.
    pub async fn replay_today(dir: &std::path::Path) -> Vec<AuditEntry> {
        let date = Utc::now().format("%Y-%m-%d");
        let path = dir.join(format!("{date}.csv"));
        let raw = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        audit_csv::parse_rows(&raw)
    }

    /// Flush+fsync the file buffer. Call periodically and at shutdown.
    pub async fn flush(&self) {
        let mut g = self.inner.lock().await;
        if !g.dirty {
            return;
        }
        if let Some(file) = g.file.as_mut() {
            if let Err(e) = file.flush().await {
                warn!("audit: flush failed: {e}");
                return;
            }
            if let Err(e) = file.sync_data().await {
                warn!("audit: fsync failed: {e}");
                return;
            }
        }
        g.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::orders::{OrderAction, OrderRequest};
    use libs::protocol::ExchangeName;
    use tempfile::tempdir;

    #[tokio::test]
    async fn writes_csv_row() {
        let dir = tempdir().unwrap();
        let sink = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        sink.log_request(&OrderRequest {
            req_id: 1,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Heartbeat,
        })
        .await;
        sink.flush().await;
        let date = chrono::Utc::now().format("%Y-%m-%d");
        let path = dir.path().join(format!("{date}.csv"));
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut lines = raw.lines();
        assert!(lines
            .next()
            .unwrap()
            .starts_with("seq,prev_hash,ts_iso,ts_ns,kind,"));
        let row = lines.next().expect("data row");
        assert!(row.contains("request"));
        assert!(row.contains("heartbeat"));
        assert!(row.contains("kalshi"));
    }

    #[tokio::test]
    async fn prev_hash_chain_advances() {
        let dir = tempdir().unwrap();
        let sink = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        sink.log_synthetic("a", serde_json::json!({})).await;
        let h1 = {
            let g = sink.inner.lock().await;
            g.prev_hash.clone()
        };
        sink.log_synthetic("b", serde_json::json!({})).await;
        let h2 = {
            let g = sink.inner.lock().await;
            g.prev_hash.clone()
        };
        assert_ne!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[tokio::test]
    async fn replay_round_trip() {
        let dir = tempdir().unwrap();
        let sink = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        sink.log_request(&OrderRequest {
            req_id: 7,
            exchange: ExchangeName::Polymarket,
            action: OrderAction::Heartbeat,
        })
        .await;
        sink.flush().await;
        let entries = AuditSink::replay_today(dir.path()).await;
        assert_eq!(entries.len(), 1);
        match &entries[0].payload {
            Payload::Request { req } => assert_eq!(req.req_id, 7),
            _ => panic!("expected request"),
        }
    }
}
