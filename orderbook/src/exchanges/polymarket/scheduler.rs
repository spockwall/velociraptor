//! Rolling-window scheduler for Polymarket markets.
//!
//! Given a `base_slug` and `interval_secs`, this scheduler:
//! 1. Computes the current window end (`win_end = ceil(now / interval) * interval`).
//! 2. Spawns a per-window task via a user-supplied async closure.
//! 3. Immediately kicks off an *inline* REST prefetch for the next
//!    window's `clobTokenIds` (still inside this same scheduler task —
//!    no extra spawn). The prefetch future runs concurrently with the
//!    `sleep_until(win_end)` we're about to do, so by the boundary
//!    we (almost always) have tokens in hand.
//! 4. At `win_end`, await the prefetch future, stop the old task,
//!    spawn the new one with the pre-resolved tokens. There is **no
//!    overlap** — only one window publishes on `polymarket:{base_slug}`
//!    at a time.
//! 5. Loops.
//!
//! Why no `EARLY_START_SECS` and no `tokio::spawn` for the prefetch:
//! the scheduler's own task is already async, so awaiting a sleep
//! drives any other future in scope. Kicking off the prefetch as a
//! plain future and awaiting it after the sleep gives the same wall
//! time as a separate task would, with less plumbing.

use crate::connection::SystemControl;
use crate::exchanges::polymarket::resolver::{PolymarketLabeledAsset, resolve_one_slug_async};
use libs::time::now_secs;
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// A running per-window task (one WS connection for one `full_slug`).
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

    /// Request shutdown and give the task up to 5s to flush before
    /// forcing cancellation. Detached.
    pub fn stop(self) {
        let full_slug = self.full_slug;
        let control = self.control;
        let mut handle = self.handle;

        info!(full_slug = %full_slug, "[scheduler] stopping task");
        control.shutdown();

        tokio::spawn(async move {
            tokio::select! {
                result = &mut handle => {
                    if let Err(err) = result {
                        warn!(full_slug = %full_slug, "[scheduler] task join error: {err}");
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    warn!(full_slug = %full_slug, "[scheduler] force-aborting task");
                    handle.abort();
                    let _ = handle.await;
                }
            }
        });
    }
}

/// Compute `(win_start, win_end, full_slug)` for the current window.
pub fn current_window(base_slug: &str, interval_secs: u64) -> (u64, u64, String) {
    let now = now_secs();
    let win_start = (now / interval_secs) * interval_secs;
    let win_end = win_start + interval_secs;
    let full_slug = format!("{base_slug}-{win_start}");
    (win_start, win_end, full_slug)
}

/// Run the rolling-window loop for one market.
///
/// `spawn_fn(full_slug, prefetched_tokens)` is invoked at each window
/// boundary. `prefetched_tokens` is `Some` when the scheduler already
/// resolved tokens for that slug; `None` only on the very first iteration
/// (cold start). It must return `Some(WindowTask)` on success or `None`
/// to ask the scheduler to retry the same slug 5s later.
pub async fn run_rolling_scheduler<F, Fut>(base_slug: String, interval_secs: u64, mut spawn_fn: F)
where
    F: FnMut(String, Option<Vec<PolymarketLabeledAsset>>) -> Fut + Send,
    Fut: Future<Output = Option<WindowTask>> + Send,
{
    if interval_secs == 0 {
        // Static market — start once and run forever, no prefetch.
        if let Some(task) = spawn_fn(base_slug.clone(), None).await {
            let _ = task.handle.await;
        }
        return;
    }

    let mut current: Option<WindowTask> = None;

    loop {
        let (_, win_end, full_slug) = current_window(&base_slug, interval_secs);

        // Cold start (no current task) or the loop fell behind a clock
        // tick — spawn with None and let spawn_fn do the inline fetch.
        if current
            .as_ref()
            .map(|t| t.full_slug != full_slug)
            .unwrap_or(true)
        {
            if let Some(old) = current.take() {
                old.stop();
            }
            current = spawn_fn(full_slug.clone(), None).await;
            if current.is_none() {
                warn!(full_slug = %full_slug, "[scheduler] spawn failed; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        // ── Prefetch + sleep run concurrently ────────────────────────
        // Kick off the next window's REST resolve as a plain future
        // (no `tokio::spawn`). We won't await it until after the sleep,
        // and the sleep is itself an await point — so the runtime will
        // drive both forward in parallel within this single task.
        let next_full_slug = format!("{base_slug}-{win_end}");
        let prefetch = resolve_one_slug_async(base_slug.clone(), next_full_slug.clone());

        let remaining = win_end.saturating_sub(now_secs());
        let sleep = tokio::time::sleep(Duration::from_secs(remaining));

        // tokio::join! polls both futures cooperatively. By the time
        // the sleep wakes us (`remaining` seconds), the prefetch has
        // almost certainly resolved. If it hasn't, this awaits it —
        // the public topic stays silent until tokens arrive.
        let (tokens, _) = tokio::join!(prefetch, sleep);
        if tokens.is_empty() {
            warn!(full_slug = %next_full_slug, "[scheduler] prefetch returned no tokens; falling back to inline fetch");
        }

        // Stop old; spawn new with the pre-resolved tokens. Order matters:
        // stop FIRST so only one publisher is ever on the topic.
        if let Some(old) = current.take() {
            old.stop();
        }

        let tokens = if tokens.is_empty() {
            warn!("Prefetch Token not found");
            None
        } else {
            Some(tokens)
        };
        current = spawn_fn(next_full_slug.clone(), tokens).await;
        if current.is_none() {
            warn!(full_slug = %next_full_slug, "[scheduler] next-window spawn failed; retrying top-of-loop in 5s");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}
