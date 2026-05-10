//! Graceful shutdown handler.
//!
//! Flow on SIGTERM / Ctrl-C:
//!   1. `draining = true` — gateway rejects new ROUTER frames with `Internal { "shutting_down" }`.
//!   2. Wait up to 5s for `inflight` to reach 0.
//!   3. Audit-log `shutdown_inflight_unknown` for anything still pending at deadline.
//!   4. Flush+fsync the audit file.
//!   5. Caller exits non-zero if `unresolved > 0`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde_json::json;
use tracing::{info, warn};

use crate::ops::AuditSink;

#[derive(Default)]
pub struct ShutdownState {
    pub draining: AtomicBool,
    pub inflight: AtomicU32,
}

impl ShutdownState {
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
    }
    pub fn inc_inflight(&self) {
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_inflight(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Wait for `inflight` to drain up to `timeout`. Returns the number that were
/// still pending at deadline (`0` on a clean drain).
pub async fn drain(state: Arc<ShutdownState>, audit: Arc<AuditSink>, timeout: Duration) -> u32 {
    state.draining.store(true, Ordering::Relaxed);
    let deadline = tokio::time::Instant::now() + timeout;
    info!(
        "shutdown: draining (timeout={}ms)",
        timeout.as_millis()
    );

    while tokio::time::Instant::now() < deadline {
        let n = state.inflight.load(Ordering::Relaxed);
        if n == 0 {
            audit.flush().await;
            info!("shutdown: drained cleanly");
            return 0;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let unresolved = state.inflight.load(Ordering::Relaxed);
    if unresolved > 0 {
        warn!("shutdown: {unresolved} requests still inflight at deadline");
        audit
            .log_synthetic(
                "shutdown_inflight_unknown",
                json!({ "count": unresolved }),
            )
            .await;
    }
    audit.flush().await;
    unresolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn clean_drain_returns_zero() {
        let dir = tempdir().unwrap();
        let audit = Arc::new(
            AuditSink::open(dir.path().to_path_buf(), None, 1000)
                .await
                .unwrap(),
        );
        let state = Arc::new(ShutdownState::default());
        let n = drain(state, audit, Duration::from_millis(50)).await;
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn deadline_reports_unresolved() {
        let dir = tempdir().unwrap();
        let audit = Arc::new(
            AuditSink::open(dir.path().to_path_buf(), None, 1000)
                .await
                .unwrap(),
        );
        let state = Arc::new(ShutdownState::default());
        state.inc_inflight();
        state.inc_inflight();
        let n = drain(state, audit, Duration::from_millis(50)).await;
        assert_eq!(n, 2);
    }
}
