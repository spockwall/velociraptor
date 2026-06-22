//! Shared Redis helper: key schema + `RedisHandle` + event-list spillover.
//!
//! All services that write to Redis go through this module so the key
//! layout stays consistent.

pub mod keys;
pub mod spillover;

use anyhow::Result;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tracing::error;

/// One Redis key with its type and element count, as surfaced by
/// [`RedisHandle::key_overview`]. `size` is the element count for
/// list/set/hash/zset/stream; `None` for scalar types (string) whose "size"
/// isn't a count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisKeyInfo {
    pub key: String,
    /// Redis type: string / list / set / hash / zset / stream / none.
    pub kind: String,
    /// Element count (LLEN/SCARD/HLEN/ZCARD/XLEN). `None` for strings.
    pub size: Option<u64>,
    /// Optional decoded payload for human-readable key families (e.g. `bba:`
    /// best bid/ask, `window_open_price:` price). `None` for keys we don't
    /// decode. Populated by the backend route, not `key_overview` itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A cheaply-clonable handle to Redis, backed by `ConnectionManager` so
/// reconnects are automatic.
#[derive(Clone)]
pub struct RedisHandle {
    conn: ConnectionManager,
    /// Max entries kept in each capped event list.
    pub event_list_cap: usize,
}

impl RedisHandle {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1:6379`).
    pub async fn connect(url: &str, event_list_cap: usize) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = ConnectionManager::new(client).await?;
        Ok(Self { conn, event_list_cap })
    }

    /// Raw access to the underlying connection manager.
    pub fn raw(&self) -> ConnectionManager {
        self.conn.clone()
    }

    /// Enumerate every key with its type and element count for a live overview
    /// (used by the backend's `/api/redis/keys`). Uses cursor-based `SCAN`
    /// (never `KEYS`) so it doesn't block Redis, then fetches each key's `TYPE`
    /// and its size with the type-appropriate command. Best-effort: on any
    /// error it returns what it has gathered so far. Results are sorted by key.
    pub async fn key_overview(&self) -> Vec<RedisKeyInfo> {
        let mut conn = self.conn.clone();
        let mut out: Vec<RedisKeyInfo> = Vec::new();
        let mut cursor: u64 = 0;

        loop {
            let scan: redis::RedisResult<(u64, Vec<String>)> = redis::cmd("SCAN")
                .arg(cursor)
                .arg("COUNT")
                .arg(512)
                .query_async(&mut conn)
                .await;
            let (next, keys) = match scan {
                Ok(v) => v,
                Err(e) => {
                    error!("Redis SCAN failed: {e}");
                    break;
                }
            };

            for key in keys {
                let kind: String = redis::cmd("TYPE")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or_else(|_| "unknown".to_string());

                let size_cmd = match kind.as_str() {
                    "list" => Some("LLEN"),
                    "set" => Some("SCARD"),
                    "hash" => Some("HLEN"),
                    "zset" => Some("ZCARD"),
                    "stream" => Some("XLEN"),
                    _ => None, // string / none → no element count
                };
                let size = match size_cmd {
                    Some(cmd) => redis::cmd(cmd)
                        .arg(&key)
                        .query_async::<u64>(&mut conn)
                        .await
                        .ok(),
                    None => None,
                };

                out.push(RedisKeyInfo {
                    key,
                    kind,
                    size,
                    data: None,
                });
            }

            cursor = next;
            if cursor == 0 {
                break;
            }
        }

        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    // ── Orderbook state ───────────────────────────────────────────────────────

    /// Overwrite the full orderbook snapshot for `(exchange, symbol)`.
    /// Payload is msgpack bytes.
    pub async fn set_orderbook(&self, exchange: &str, symbol: &str, payload: &[u8]) {
        let key = keys::RedisKey::orderbook(exchange, symbol);
        if let Err(e) = self.conn.clone().set::<_, _, ()>(&key, payload).await {
            error!("Redis SET {key} failed: {e}");
        }
    }

    /// Overwrite the BBA (best-bid-ask) snapshot for `(exchange, symbol)`.
    /// Payload is msgpack bytes.
    pub async fn set_bba(&self, exchange: &str, symbol: &str, payload: &[u8]) {
        let key = keys::RedisKey::bba(exchange, symbol);
        if let Err(e) = self.conn.clone().set::<_, _, ()>(&key, payload).await {
            error!("Redis SET {key} failed: {e}");
        }
    }

    // ── Read helpers ──────────────────────────────────────────────────────────

    /// GET a raw msgpack blob from `key`. Returns `None` if the key is absent.
    pub async fn get_raw(&self, key: &str) -> Option<Vec<u8>> {
        self.conn
            .clone()
            .get::<_, Option<Vec<u8>>>(key)
            .await
            .map_err(|e| error!("Redis GET {key} failed: {e}"))
            .ok()
            .flatten()
    }

    /// LRANGE `key` from `start` to `stop` (inclusive), returning raw msgpack blobs.
    pub async fn lrange_raw(&self, key: &str, start: isize, stop: isize) -> Vec<Vec<u8>> {
        self.conn
            .clone()
            .lrange::<_, Vec<Vec<u8>>>(key, start, stop)
            .await
            .map_err(|e| error!("Redis LRANGE {key} failed: {e}"))
            .unwrap_or_default()
    }

    /// HGETALL `key`. Returns empty map on missing key or error.
    pub async fn hgetall(&self, key: &str) -> std::collections::HashMap<String, String> {
        self.conn
            .clone()
            .hgetall::<_, std::collections::HashMap<String, String>>(key)
            .await
            .map_err(|e| error!("Redis HGETALL {key} failed: {e}"))
            .unwrap_or_default()
    }

    /// HSET multiple fields at `key` in one round-trip.
    pub async fn hset_multi(&self, key: &str, fields: &[(&str, &str)]) {
        if fields.is_empty() {
            return;
        }
        let pairs: Vec<(&str, &str)> = fields.to_vec();
        if let Err(e) = self
            .conn
            .clone()
            .hset_multiple::<_, _, _, ()>(key, &pairs)
            .await
        {
            error!("Redis HSET {key} failed: {e}");
        }
    }

    /// SADD `member` to set `key`.
    pub async fn sadd(&self, key: &str, member: &str) {
        if let Err(e) = self.conn.clone().sadd::<_, _, ()>(key, member).await {
            error!("Redis SADD {key} failed: {e}");
        }
    }

    /// SREM `member` from set `key`.
    pub async fn srem(&self, key: &str, member: &str) {
        if let Err(e) = self.conn.clone().srem::<_, _, ()>(key, member).await {
            error!("Redis SREM {key} failed: {e}");
        }
    }

    /// SMEMBERS `key`. Returns empty vec on missing key or error.
    pub async fn smembers(&self, key: &str) -> Vec<String> {
        self.conn
            .clone()
            .smembers::<_, Vec<String>>(key)
            .await
            .map_err(|e| error!("Redis SMEMBERS {key} failed: {e}"))
            .unwrap_or_default()
    }

    /// DEL `key`.
    pub async fn del(&self, key: &str) {
        if let Err(e) = self.conn.clone().del::<_, ()>(key).await {
            error!("Redis DEL {key} failed: {e}");
        }
    }

    // ── Capped event lists ────────────────────────────────────────────────────

    /// Prepend `payload` to the capped list at `list_key`, trimming it to
    /// `cap` entries. Uses a pipeline for atomicity.
    pub async fn lpush_capped(&self, list_key: &str, payload: &[u8], cap: usize) {
        let cap = cap as isize;
        let mut pipe = redis::pipe();
        pipe.lpush(list_key, payload)
            .ltrim(list_key, 0, cap - 1)
            .ignore();
        if let Err(e) = pipe
            .query_async::<()>(&mut self.conn.clone())
            .await
        {
            error!("Redis LPUSH+LTRIM {list_key} failed: {e}");
        }
    }
}
