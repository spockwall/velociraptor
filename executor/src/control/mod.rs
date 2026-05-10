//! Control plane.
//!
//! Combines two concerns:
//! - Redis-backed kill switch / cancel-all trigger / backend dead-man (this file)
//! - Graceful shutdown drain handler (`shutdown` submodule, re-exported below)
//!
//! Watches:
//!   - `executor:kill_switch`           — global kill flag (poll @ 250ms)
//!   - `executor:kill_switch:{exchange}` — per-exchange flag
//!   - `executor:cancel_all`             — one-shot trigger; consumed (DEL) after firing
//!   - `executor:backend_heartbeat`      — dead-man switch; if stale >30s, engage local kill
//!
//! `ControlState` is the single source of truth read by the gateway. The
//! watcher task mutates it; readers are lock-free `AtomicBool`s + a `DashMap`.

pub mod shutdown;
pub use shutdown::{drain, ShutdownState};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use libs::protocol::ExchangeName;
use libs::redis_client::RedisHandle;
use libs::redis_client::keys::Executor;
use redis::AsyncCommands;
use tracing::{info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEADMAN_THRESHOLD_SECS: i64 = 30;

pub struct ControlState {
    pub kill_switch: AtomicBool,
    pub deadman_engaged: AtomicBool,
    pub per_exchange_kill: DashMap<ExchangeName, bool>,
    /// Last seen `executor:backend_heartbeat` value (unix seconds).
    pub last_heartbeat_secs: AtomicI64,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            kill_switch: AtomicBool::new(false),
            deadman_engaged: AtomicBool::new(false),
            per_exchange_kill: DashMap::new(),
            last_heartbeat_secs: AtomicI64::new(0),
        }
    }
}

impl ControlState {
    /// Effective gate: blocks all non-cancel actions if any of:
    ///   - global kill set
    ///   - dead-man engaged
    ///   - per-exchange kill set
    pub fn is_blocked(&self, exchange: ExchangeName) -> bool {
        if self.kill_switch.load(Ordering::Relaxed) {
            return true;
        }
        if self.deadman_engaged.load(Ordering::Relaxed) {
            return true;
        }
        if let Some(v) = self.per_exchange_kill.get(&exchange) {
            return *v;
        }
        false
    }
}

/// Long-running watcher task. Returns when `shutdown` flips to `true`.
///
/// `on_cancel_all` is invoked once per detected `executor:cancel_all` trigger;
/// the watcher then `DEL`s the key. The callback should return after enqueuing
/// the cancel-all on every configured client (it does not need to wait for ack).
pub async fn run_watcher<F, Fut>(
    state: Arc<ControlState>,
    redis: RedisHandle,
    exchanges: Vec<ExchangeName>,
    shutdown: Arc<AtomicBool>,
    on_cancel_all: F,
) where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = ()> + Send,
{
    info!("control: watcher started ({} exchanges)", exchanges.len());

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut conn = redis.raw();

        // Global kill.
        let v: Option<String> = conn.get(Executor::KILL_SWITCH).await.ok().flatten();
        let kill = v.as_deref() == Some("1");
        let prev = state.kill_switch.swap(kill, Ordering::Relaxed);
        if kill != prev {
            info!("control: kill_switch -> {}", kill);
        }

        // Per-exchange kills.
        for ex in &exchanges {
            let key = Executor::kill_switch_exchange(ex.to_str());
            let v: Option<String> = conn.get(&key).await.ok().flatten();
            state.per_exchange_kill.insert(*ex, v.as_deref() == Some("1"));
        }

        // Cancel-all trigger (one-shot; consume after firing).
        let v: Option<String> = conn.get(Executor::CANCEL_ALL).await.ok().flatten();
        if v.as_deref() == Some("1") {
            info!("control: cancel_all triggered");
            on_cancel_all().await;
            if let Err(e) = conn.del::<_, ()>(Executor::CANCEL_ALL).await {
                warn!("control: failed to DEL cancel_all key: {e}");
            }
        }

        // Backend dead-man.
        let v: Option<String> = conn.get(Executor::BACKEND_HEARTBEAT).await.ok().flatten();
        if let Some(s) = v.as_deref().and_then(|s| s.parse::<i64>().ok()) {
            state.last_heartbeat_secs.store(s, Ordering::Relaxed);
            let now = chrono::Utc::now().timestamp();
            let stale = (now - s) > DEADMAN_THRESHOLD_SECS;
            let prev = state.deadman_engaged.swap(stale, Ordering::Relaxed);
            if stale && !prev {
                warn!(
                    "control: backend heartbeat stale ({}s old) — engaging local kill-switch",
                    now - s
                );
            } else if !stale && prev {
                info!("control: backend heartbeat recovered");
            }
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    info!("control: watcher stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_when_global_kill() {
        let s = ControlState::default();
        assert!(!s.is_blocked(ExchangeName::Kalshi));
        s.kill_switch.store(true, Ordering::Relaxed);
        assert!(s.is_blocked(ExchangeName::Kalshi));
        assert!(s.is_blocked(ExchangeName::Polymarket));
    }

    #[test]
    fn blocked_per_exchange() {
        let s = ControlState::default();
        s.per_exchange_kill.insert(ExchangeName::Kalshi, true);
        assert!(s.is_blocked(ExchangeName::Kalshi));
        assert!(!s.is_blocked(ExchangeName::Polymarket));
    }

    #[test]
    fn deadman_blocks_all() {
        let s = ControlState::default();
        s.deadman_engaged.store(true, Ordering::Relaxed);
        assert!(s.is_blocked(ExchangeName::Kalshi));
    }
}
