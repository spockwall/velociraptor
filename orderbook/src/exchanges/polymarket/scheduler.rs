//! Rolling-window scheduler for Polymarket markets.
//!
//! Given a `base_slug` and a `interval_secs`, this scheduler:
//! 1. Computes the current window end (`win_end = ceil(now / interval) * interval`).
//! 2. Spawns a per-window task via a user-supplied async closure.
//! 3. `EARLY_START_SECS` before `win_end`, pre-starts the next window's task.
//! 4. When `win_end` passes, stops the old task; the next task becomes current.
//! 5. Loops.
//!
//! The scheduler owns no exchange-specific logic — the caller's `spawn_fn`
//! does that (market channel, user channel, etc.). The scheduler only owns
//! the timing and the lifecycle of `WindowTask` handles.

use crate::connection::SystemControl;
use libs::time::now_secs;
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinHandle;

/// How many seconds before a window ends to pre-start the next window's task.
pub const EARLY_START_SECS: u64 = 10;

/// A running per-window task (one connection for one `full_slug`).
pub struct WindowTask {
    pub full_slug: String,
    pub control: SystemControl,
    pub handle: JoinHandle<()>,
}

impl WindowTask {
    pub fn new(full_slug: String, control: SystemControl, handle: JoinHandle<()>) -> Self {
        Self {
            full_slug,
            control,
            handle,
        }
    }

    /// Request shutdown and give the task a moment to flush/cleanup before
    /// forcing cancellation.
    pub fn stop(self) {
        let full_slug = self.full_slug;
        let control = self.control;
        let mut handle = self.handle;

        eprintln!("[scheduler] stopping task for {full_slug}");
        control.shutdown();

        tokio::spawn(async move {
            tokio::select! {
                result = &mut handle => {
                    if let Err(err) = result {
                        eprintln!("[scheduler] task join error for {full_slug}: {err}");
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    eprintln!("[scheduler] force-aborting task for {full_slug}");
                    handle.abort();
                    let _ = handle.await;
                }
            }
        });
    }
}

/// Compute `(win_start, win_end, full_slug)` for the current window.
///
/// `full_slug` is `base_slug-{win_start}` to match the convention used by the
/// Polymarket Gamma API's rolling-market slugs.
pub fn current_window(base_slug: &str, interval_secs: u64) -> (u64, u64, String) {
    let now = now_secs();
    let win_start = (now / interval_secs) * interval_secs;
    let win_end = win_start + interval_secs;
    let full_slug = format!("{base_slug}-{win_start}");
    (win_start, win_end, full_slug)
}

/// Run the rolling-window loop for one market.
///
/// `spawn_fn(full_slug)` is invoked each time a new window must be started;
/// it returns `Some(WindowTask)` on success or `None` if the window could
/// not be started (e.g. slug not yet resolvable — we retry on the next iteration).
///
/// Runs forever; returns only if `spawn_fn` panics or the future is dropped.
/// Spawn it with `tokio::spawn(run_rolling_scheduler(...))`.
pub async fn run_rolling_scheduler<F, Fut>(base_slug: String, interval_secs: u64, mut spawn_fn: F)
where
    F: FnMut(String) -> Fut + Send,
    Fut: Future<Output = Option<WindowTask>> + Send,
{
    if interval_secs == 0 {
        // Static market — start once and run forever.
        if let Some(task) = spawn_fn(base_slug.clone()).await {
            let _ = task.handle.await;
        }
        return;
    }

    let mut current: Option<WindowTask> = None;

    loop {
        let (_, win_end, full_slug) = current_window(&base_slug, interval_secs);

        // Start current window if not already running.
        let needs_spawn = current
            .as_ref()
            .map(|t| t.full_slug != full_slug)
            .unwrap_or(true);
        if needs_spawn {
            if let Some(old) = current.take() {
                old.stop();
            }
            current = spawn_fn(full_slug.clone()).await;
            if current.is_none() {
                eprintln!("[scheduler] spawn failed for {full_slug}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        // Sleep until EARLY_START_SECS before this window ends.
        let secs_until_early = win_end
            .saturating_sub(now_secs())
            .saturating_sub(EARLY_START_SECS);
        if secs_until_early > 0 {
            tokio::time::sleep(Duration::from_secs(secs_until_early)).await;
        }

        // Pre-start next window.
        let next_win_start = win_end;
        let next_full_slug = format!("{base_slug}-{next_win_start}");
        eprintln!("[scheduler] pre-starting next window: {next_full_slug}");
        let next_task = spawn_fn(next_full_slug).await;

        // Wait for current window to fully expire.
        let remaining = win_end.saturating_sub(now_secs());
        if remaining > 0 {
            tokio::time::sleep(Duration::from_secs(remaining)).await;
        }

        // Stop old; promote next.
        if let Some(old) = current.take() {
            old.stop();
        }
        current = next_task;
    }
}
