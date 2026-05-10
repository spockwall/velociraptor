//! Dual-sink audit log: redis stream + on-disk length-prefixed mpack file.
//!
//! Each entry carries:
//!   - `seq`: monotonic counter (per executor process).
//!   - `prev_hash`: BLAKE3 of the previous entry's encoded bytes (32 bytes,
//!     hex-encoded as a string for serde simplicity).
//!   - `ts_ns`: chrono::Utc::now timestamp in nanos.
//!   - `payload`: tagged enum (`Request` / `Response` / `Synthetic`).
//!
//! Both sinks see the *same* encoded bytes; encode-once, fan out.
//! Sink failures are logged at `warn!` and do not block the order path.

use std::path::PathBuf;

use chrono::Utc;
use libs::protocol::orders::{OrderError, OrderRequest, OrderResult};
use libs::redis_client::RedisHandle;
use libs::redis_client::keys::Executor;
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::warn;

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
    /// True if any non-critical entry is buffered and a flush is pending.
    dirty: bool,
}

pub struct AuditSink {
    redis: Option<RedisHandle>,
    stream_cap: usize,
    inner: tokio::sync::Mutex<State>,
}

impl AuditSink {
    /// Open `dir/{date}.mpack` and connect to the redis stream. Either sink
    /// may be absent (passing `redis: None` skips the stream sink).
    pub async fn open(
        dir: PathBuf,
        redis: Option<RedisHandle>,
        stream_cap: usize,
    ) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&dir).await?;
        let date = Utc::now().format("%Y-%m-%d");
        let path = dir.join(format!("{date}.mpack"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
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

    /// Append a `Response` entry. `critical` triggers fsync (e.g. on errors).
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

    /// Append a `Synthetic` event (kill-switch transitions, cancel-all
    /// triggers, reconciliation). Always fsynced.
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
        let entry = AuditEntry {
            seq: g.seq,
            prev_hash: g.prev_hash.clone(),
            ts_ns: Utc::now().timestamp_nanos_opt().unwrap_or(0),
            payload,
        };
        let bytes = match rmp_serde::to_vec_named(&entry) {
            Ok(b) => b,
            Err(e) => {
                warn!("audit: encode failed: {e}");
                return;
            }
        };
        // Update chain state for the next entry.
        g.seq += 1;
        g.prev_hash = blake3::hash(&bytes).to_hex().to_string();

        // Sink 1: redis stream — issue XADD via raw command since the workspace
        // redis crate isn't built with the `streams` feature.
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
                .arg(bytes.as_slice())
                .query_async::<String>(&mut conn)
                .await;
            if let Err(e) = res {
                warn!("audit: redis XADD failed: {e}");
            }
        }

        // Sink 2: append-only mpack file (length-prefixed u32 LE | bytes).
        if let Some(file) = g.file.as_mut() {
            let mut frame = Vec::with_capacity(4 + bytes.len());
            frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            frame.extend_from_slice(&bytes);
            if let Err(e) = file.write_all(&frame).await {
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
    use libs::protocol::ExchangeName;
    use libs::protocol::orders::{OrderAction, OrderRequest};
    use tempfile::tempdir;

    #[tokio::test]
    async fn writes_file_frame() {
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
        let path = dir.path().join(format!("{date}.mpack"));
        let raw = std::fs::read(&path).unwrap();
        // 4-byte length prefix + at least 1 mpack byte.
        assert!(raw.len() > 4);
        let len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        assert_eq!(raw.len(), 4 + len);
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
}
