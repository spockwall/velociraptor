//! Shared Redis helper: key schema + `RedisHandle` + event-list spillover.
//!
//! All services that write to Redis go through this module so the key
//! layout stays consistent. See `docs/redis.md` and the plan for rationale.
//!
//! Full implementation arrives in Phase 5 — this module currently provides
//! the type surface and key helpers so downstream crates can compile against
//! a stable API.

pub mod keys;
pub mod spillover;

use anyhow::Result;
use redis::aio::ConnectionManager;

/// A cheaply-clonable handle to Redis, backed by `ConnectionManager` so
/// reconnects are automatic.
#[derive(Clone)]
pub struct RedisHandle {
    conn: ConnectionManager,
}

impl RedisHandle {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1:6379`).
    pub async fn connect(url: &str) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = ConnectionManager::new(client).await?;
        Ok(Self { conn })
    }

    /// Raw access to the underlying connection manager. Prefer the typed
    /// helpers (Phase 5) once they exist.
    pub fn raw(&self) -> ConnectionManager {
        self.conn.clone()
    }
}
