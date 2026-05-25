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
//!   - `executor:reload_config`          — one-shot trigger; consumed (DEL) after firing
//!   - `executor:backend_heartbeat`      — dead-man switch; if stale >30s, engage local kill
//!
//! This module owns **only operational signals**. Risk-domain state
//! (open-order counts, place-time windows, the active `RiskConfig`)
//! lives in `pretrade::PretradeState`. The watcher invokes the executor
//! via [`ControlCallbacks`] for the two actions that cross the boundary
//! (cancel-all + reload-risk) so the control thread remains genuinely
//! standalone.

pub mod shutdown;
pub use shutdown::{drain, ShutdownState};

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use libs::protocol::ExchangeName;
use libs::redis_client::keys::Executor;
use libs::redis_client::RedisHandle;
use redis::AsyncCommands;
use tracing::{info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEADMAN_THRESHOLD_SECS: i64 = 30;

/// Operational signals only. No risk-domain state.
#[derive(Default)]
pub struct ControlState {
    pub kill_switch: AtomicBool,
    pub paused: AtomicBool,
    pub deadman_engaged: AtomicBool,
    pub per_exchange_kill: DashMap<ExchangeName, bool>,
    /// Last seen `executor:backend_heartbeat` value (unix seconds).
    pub last_heartbeat_secs: AtomicI64,
}

impl ControlState {
    /// Effective gate: blocks all non-cancel actions if any of:
    ///   - global kill set
    ///   - paused
    ///   - dead-man engaged
    ///   - per-exchange kill set
    pub fn is_blocked(&self, exchange: ExchangeName) -> bool {
        if self.kill_switch.load(Ordering::Relaxed) {
            return true;
        }
        if self.paused.load(Ordering::Relaxed) {
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

/// What the watcher calls back into when a one-shot control key fires.
///
/// Implementations are not assumed to be cheap and the watcher does NOT
/// await per-call completion guarantees beyond the boundary of one tick:
/// if a callback hangs, the watcher still advances on the next iteration.
#[async_trait]
pub trait ControlCallbacks: Send + Sync {
    /// `executor:cancel_all` fired. Implementation should flatten every
    /// exchange and clear any in-process open-order bookkeeping.
    async fn cancel_all_now(&self);
    /// `executor:reload_config` fired. Implementation should re-parse the
    /// YAML and swap the active risk limits.
    async fn reload_config(&self);
}

/// Long-running watcher task. Returns when `shutdown` flips to `true`.
pub async fn run_watcher(
    state: Arc<ControlState>,
    redis: RedisHandle,
    exchanges: Vec<ExchangeName>,
    shutdown: Arc<AtomicBool>,
    callbacks: Arc<dyn ControlCallbacks>,
) {
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

        // Pause (separate signal — kill is "block + cancel", pause is
        // "block only"; both surface through is_blocked).
        let v: Option<String> = conn.get(Executor::PAUSE).await.ok().flatten();
        let paused = v.as_deref() == Some("1");
        let prev = state.paused.swap(paused, Ordering::Relaxed);
        if paused != prev {
            info!("control: paused -> {}", paused);
        }

        // Per-exchange kills.
        for ex in &exchanges {
            let key = Executor::kill_switch_exchange(ex.to_str());
            let v: Option<String> = conn.get(&key).await.ok().flatten();
            state
                .per_exchange_kill
                .insert(*ex, v.as_deref() == Some("1"));
        }

        // Cancel-all trigger (one-shot; consume after firing).
        let v: Option<String> = conn.get(Executor::CANCEL_ALL).await.ok().flatten();
        if v.as_deref() == Some("1") {
            info!("control: cancel_all triggered");
            callbacks.cancel_all_now().await;
            if let Err(e) = conn.del::<_, ()>(Executor::CANCEL_ALL).await {
                warn!("control: failed to DEL cancel_all key: {e}");
            }
        }

        // Config reload trigger (one-shot).
        let v: Option<String> = conn.get(Executor::RELOAD_CONFIG).await.ok().flatten();
        if v.as_deref() == Some("1") {
            info!("control: reload_config triggered");
            callbacks.reload_config().await;
            if let Err(e) = conn.del::<_, ()>(Executor::RELOAD_CONFIG).await {
                warn!("control: failed to DEL reload_config key: {e}");
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
    fn blocked_when_paused() {
        let s = ControlState::default();
        s.paused.store(true, Ordering::Relaxed);
        assert!(s.is_blocked(ExchangeName::Kalshi));
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
