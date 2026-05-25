//! Rolling-window scheduler for Kalshi 15-minute markets.
//!
//! Kalshi binary markets rotate on UTC :00/:15/:30/:45 boundaries.
//! Given a `series` name (e.g. `"KXBTC15M"`), this scheduler:
//! 1. Computes the current window's close time and builds the market
//!    ticker (deterministic, no REST round-trip — see
//!    [`crate::exchanges::kalshi::utils::build_market_ticker`]).
//! 2. Spawns a per-window task via a user-supplied async closure.
//! 3. Sleeps until the close.
//! 4. Stops the old task and spawns the next window's task. There is
//!    **no overlap** — only one window publishes on the public Kalshi
//!    topic at a time. Kalshi's ticker is computed locally so there is
//!    nothing to prefetch; the WS handshake at the boundary is the only
//!    cost, ~500ms-1s of silence per boundary.
//! 5. Loops.
//!
//! The scheduler owns no exchange-specific WS logic — the caller's
//! `spawn_fn` does that. The scheduler owns the timing and the
//! lifecycle of `WindowTask` handles.

use crate::connection::SystemControl;
use crate::exchanges::kalshi::utils::{build_market_ticker, current_window_close};
use chrono::{Duration, Utc};
use std::future::Future;
use std::time::Duration as StdDuration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// A running per-window task (one connection for one market ticker).
pub struct WindowTask {
    pub ticker: String,
    pub control: SystemControl,
    pub handle: JoinHandle<()>,
}

impl WindowTask {
    pub fn new(ticker: String, control: SystemControl, handle: JoinHandle<()>) -> Self {
        Self {
            ticker,
            control,
            handle,
        }
    }

    pub fn stop(self) {
        info!(ticker = %self.ticker, "[kalshi-scheduler] stopping task");
        self.control.shutdown();
        self.handle.abort();
    }
}

/// Run the 15-minute rolling-window loop for one Kalshi series.
///
/// `spawn_fn(ticker)` is called each time a new window must be started;
/// it receives the full market ticker (e.g. `"KXBTC15M-26APR160700-00"`)
/// and returns `Some(WindowTask)` on success or `None` if the window
/// could not be started (we retry 5s later).
pub async fn run_rolling_scheduler<F, Fut>(series: String, mut spawn_fn: F)
where
    F: FnMut(String) -> Fut + Send,
    Fut: Future<Output = Option<WindowTask>> + Send,
{
    let mut current: Option<WindowTask> = None;

    loop {
        let win_close = current_window_close(Utc::now());
        let win_close_unix = win_close.timestamp() as u64;
        let ticker = build_market_ticker(&series, win_close);

        // Spawn current window if not already running.
        if current.as_ref().map(|t| t.ticker != ticker).unwrap_or(true) {
            if let Some(old) = current.take() {
                old.stop();
            }
            current = spawn_fn(ticker.clone()).await;
            if current.is_none() {
                warn!(ticker = %ticker, "[kalshi-scheduler] spawn failed; retrying in 5s");
                tokio::time::sleep(StdDuration::from_secs(5)).await;
                continue;
            }
        }

        // Sleep until the boundary.
        let remaining = win_close_unix.saturating_sub(Utc::now().timestamp() as u64);
        if remaining > 0 {
            tokio::time::sleep(StdDuration::from_secs(remaining)).await;
        }

        // Boundary: stop old, spawn new. Order matters — stop FIRST so
        // only one publisher is ever on the topic.
        let next_close = win_close + Duration::minutes(15);
        let next_ticker = build_market_ticker(&series, next_close);
        if let Some(old) = current.take() {
            old.stop();
        }
        current = spawn_fn(next_ticker.clone()).await;
        if current.is_none() {
            warn!(ticker = %next_ticker, "[kalshi-scheduler] next-window spawn failed; retrying top-of-loop in 5s");
            tokio::time::sleep(StdDuration::from_secs(5)).await;
        }
    }
}
