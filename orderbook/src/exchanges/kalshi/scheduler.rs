//! Rolling-window scheduler for Kalshi 15-minute markets.
//!
//! Kalshi binary markets rotate on UTC :00/:15/:30/:45 boundaries. Given a
//! series name (e.g. `"KXBTC15M"`), this scheduler:
//! 1. Computes the current window's close time and builds the market ticker.
//! 2. Spawns a per-window task via a user-supplied async closure.
//! 3. `EARLY_START_SECS` before the close, pre-starts the next window's task.
//! 4. When the window expires, stops the old task; the next task becomes current.
//! 5. Loops.
//!
//! The scheduler owns no exchange-specific logic — the caller's `spawn_fn`
//! does that (credentials, subscription message, hooks, etc.). The scheduler
//! only owns the timing and the lifecycle of `WindowTask` handles.

use crate::connection::SystemControl;
use crate::exchanges::kalshi::utils::{build_market_ticker, current_window_close};
use chrono::{Duration, Utc};
use std::future::Future;
use std::time::Duration as StdDuration;
use tokio::task::JoinHandle;

/// How many seconds before a window closes to pre-start the next window's task.
pub const EARLY_START_SECS: u64 = 10;

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
        eprintln!("[kalshi-scheduler] stopping task for {}", self.ticker);
        self.control.shutdown();
        self.handle.abort();
    }
}

/// Run the 15-minute rolling-window loop for one Kalshi series.
///
/// `spawn_fn(ticker)` is called each time a new window must be started; it
/// receives the full market ticker (e.g. `"KXBTC15M-26APR160700-00"`) and
/// returns `Some(WindowTask)` on success or `None` if the window could not be
/// started (we retry on the next loop iteration).
///
/// Runs forever; drop the returned future or abort the `JoinHandle` to stop.
/// Typical usage: `tokio::spawn(run_rolling_scheduler(series, spawn_fn))`.
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

        // Connect if this is the first iteration or the window rolled over.
        let needs_connect = current.as_ref().map(|t| t.ticker != ticker).unwrap_or(true);
        if needs_connect {
            if let Some(old) = current.take() {
                old.stop();
            }
            current = spawn_fn(ticker.clone()).await;
            if current.is_none() {
                eprintln!("[kalshi-scheduler] spawn failed for {ticker}; retrying in 5s");
                tokio::time::sleep(StdDuration::from_secs(5)).await;
                continue;
            }
        }

        // Sleep until EARLY_START_SECS before the window closes.
        let secs_until_early = win_close_unix
            .saturating_sub(Utc::now().timestamp() as u64)
            .saturating_sub(EARLY_START_SECS);
        if secs_until_early > 0 {
            tokio::time::sleep(StdDuration::from_secs(secs_until_early)).await;
        }

        // Pre-start next window.
        let next_close = win_close + Duration::minutes(15);
        let next_ticker = build_market_ticker(&series, next_close);
        eprintln!("[kalshi-scheduler] pre-starting next window: {next_ticker}");
        let next_task = spawn_fn(next_ticker).await;

        // Wait for the current window to fully expire.
        let remaining = win_close_unix.saturating_sub(Utc::now().timestamp() as u64);
        if remaining > 0 {
            tokio::time::sleep(StdDuration::from_secs(remaining)).await;
        }

        // Stop old; promote next.
        if let Some(old) = current.take() {
            old.stop();
        }
        current = next_task;
    }
}
