//! Shared Redis helper: key schema + `RedisHandle` + event-list spillover.
//!
//! All services that write to Redis go through this module so the key
//! layout stays consistent.

pub mod keys;
pub mod spillover;

use anyhow::Result;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use tracing::error;

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

    // ── Account state ─────────────────────────────────────────────────────────

    /// Overwrite the position for `(exchange, symbol)`. Payload is msgpack bytes.
    pub async fn set_position(&self, exchange: &str, symbol: &str, payload: &[u8]) {
        let key = keys::RedisKey::position(exchange, symbol);
        if let Err(e) = self.conn.clone().set::<_, _, ()>(&key, payload).await {
            error!("Redis SET {key} failed: {e}");
        }
    }

    /// Overwrite the balance for `(exchange, asset)`. Payload is msgpack bytes.
    pub async fn set_balance(&self, exchange: &str, asset: &str, payload: &[u8]) {
        let key = keys::RedisKey::balance(exchange, asset);
        if let Err(e) = self.conn.clone().set::<_, _, ()>(&key, payload).await {
            error!("Redis SET {key} failed: {e}");
        }
    }

    // ── Capped event lists ────────────────────────────────────────────────────

    /// Prepend `payload` to the capped list at `list_key`, trimming it to
    /// `self.event_list_cap` entries. Uses a pipeline for atomicity.
    pub async fn lpush_capped(&self, list_key: &str, payload: &[u8]) {
        let cap = self.event_list_cap as isize;
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
