//! Helpers shared by the Polymarket and Kalshi rolling-window plumbing.
//!
//! Polymarket Up/Down and Kalshi 15-min markets follow the same lifecycle:
//!
//! 1. Resolve the current window's identifiers (asset_ids / ticker).
//! 2. Evict any stale labels left over from a window that ended without a
//!    clean shutdown (the live windows clean themselves up via their own
//!    watchdog; we only touch labels whose `window_start + interval_secs`
//!    is already past).
//! 3. Schedule the new window's label registration to fire at the boundary
//!    (not immediately at pre-start, ~10s before the previous window ends —
//!    otherwise `/api/{exchange}/markets` briefly returns two rows per
//!    market during the overlap).
//! 4. Build a per-window `StreamEngine` + `StreamSystem` and register a
//!    forward hook that, for every snapshot/trade, stamps the per-window
//!    identifier into `full_slug`, publishes onto the main bus as
//!    `StreamEvent::Rolling{Snapshot,LastTradePrice}` (keyed by the stable
//!    `base_slug` / `series`), and mirrors the payload into Redis under the
//!    same stable key. The forward hook also gates on `now < window_start`
//!    so the pre-started window's frames don't reach ZMQ or Redis until
//!    the boundary.
//! 5. Spawn a shutdown watchdog that drops the window's labels when the
//!    scheduler stops the window. The orderbook storage keys
//!    (`ob:/bba:/snapshots:/trades:`) are NOT touched by the watchdog —
//!    they're keyed by the stable `base_slug` / `series` and overwritten by
//!    the next window's first snapshot.
//!
//! The four shared helpers land here over the staged refactor:
//!   - `evict_stale_labels` (step 2)
//!   - `spawn_deferred_label_registration` (step 3)
//!   - `spawn_label_watchdog` (step 4)
//!   - `register_{polymarket,kalshi}_rolling_hooks` (step 5)

use crate::topics::bba::BbaPayload;
use libs::protocol::{LastTradePrice, OrderbookSnapshot};
use libs::redis_client::{keys::RedisKey, RedisHandle};
use orderbook::connection::SystemControl;
use orderbook::{StreamEngine, StreamEngineBus, StreamEvent};
use std::collections::HashMap;

/// One label to publish at the boundary.
///
/// Different venues have different `fields` shapes — Polymarket uses
/// `(base_slug, full_slug, side, window_start, interval_secs)`, Kalshi
/// uses `(series, ticker, window_start, interval_secs)`. The helper
/// is agnostic to the shape; the caller assembles the slice.
pub(super) struct LabelRegistration {
    pub label_key: String,
    pub member_id: String,
    pub fields: Vec<(&'static str, String)>,
}

/// Sweep `grouping_index_key`'s set members; drop any whose label hash says
/// the window has already ended. Active overlapping windows (a member is
/// not stale and not in `skip_ids`) are left alone — they own their own
/// cleanup via [`spawn_label_watchdog`].
///
/// "Stale" = the label's `interval_secs == 0` (legacy / missing field) OR
/// `window_start + interval_secs <= now`.
///
/// Does NOT touch the per-grouping storage keys (`ob:/bba:/snapshots:/trades:`)
/// — those are keyed by the stable `base_slug` / `series` and survive
/// rollover by design.
///
/// Arguments:
/// - `grouping_index_key`: per-base/per-series SET (e.g.
///   `polymarket:base:{slug}:assets` or `kalshi:series:{series}:tickers`).
/// - `label_index_key`: global label-index SET constant
///   (`RedisKey::POLYMARKET_LABEL_INDEX` or `KALSHI_LABEL_INDEX`).
/// - `label_key_fn`: closure mapping a member id to its label hash key
///   (`RedisKey::polymarket_label` / `kalshi_label`).
/// - `skip_ids`: member ids being (re)registered by the caller — never evict
///   these, regardless of staleness.
/// - `now`: caller's clock; passed in so callers can share a single `now_secs()`
///   sample across the rest of the spawn logic.
pub(super) async fn evict_stale_labels(
    redis: &RedisHandle,
    grouping_index_key: &str,
    label_index_key: &'static str,
    label_key_fn: impl Fn(&str) -> String,
    skip_ids: &[String],
    now: u64,
) {
    let members = redis.smembers(grouping_index_key).await;
    for id in &members {
        if skip_ids.iter().any(|s| s == id) {
            continue;
        }
        let label_key = label_key_fn(id);
        let hash = redis.hgetall(&label_key).await;
        let ws = hash
            .get("window_start")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let iv = hash
            .get("interval_secs")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let is_stale = iv == 0 || (ws > 0 && ws + iv <= now);
        if !is_stale {
            continue;
        }
        redis.del(&label_key).await;
        redis.srem(label_index_key, id).await;
        redis.srem(grouping_index_key, id).await;
    }
}

/// Spawn a task that publishes the new window's labels at the boundary.
///
/// Why deferred: pre-start spins up the new window ~10s before the
/// previous window ends. If we wrote labels immediately, both the old
/// and new window's labels would coexist in the index, and
/// `/api/{exchange}/markets` would return two rows per market during
/// the overlap. By sleeping until `window_start` first, we keep the
/// discovery API showing exactly one window per (base_slug/series, side)
/// at all times.
///
/// The pre-started window's WS connection is still live during the
/// overlap (needed for full materialisation), but its forward hook
/// gates on `now < window_start` so no frames reach ZMQ/Redis until
/// the boundary either — see the per-window forward hooks.
pub(super) fn spawn_deferred_label_registration(
    redis: RedisHandle,
    label_index_key: &'static str,
    grouping_index_key: String,
    entries: Vec<LabelRegistration>,
    window_start: u64,
) {
    let now = libs::time::now_secs();
    let delay_secs = window_start.saturating_sub(now);
    tokio::spawn(async move {
        if delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        }
        for e in &entries {
            redis.sadd(&grouping_index_key, &e.member_id).await;
        }
        for e in &entries {
            // hset_multi takes a slice of (&str, &str). Build owned refs
            // from each entry's field tuples.
            let field_refs: Vec<(&str, &str)> = e
                .fields
                .iter()
                .map(|(k, v)| (*k, v.as_str()))
                .collect();
            redis.hset_multi(&e.label_key, &field_refs).await;
            redis.sadd(label_index_key, &e.member_id).await;
        }
    });
}

/// Spawn a watchdog that polls `SystemControl::is_shutdown()` and, on
/// shutdown, drops the window's labels from Redis.
///
/// Why a separate task: the rolling scheduler's `WindowTask::stop` calls
/// `handle.abort()` on the window's main task after a short grace period.
/// `abort()` cancels anything awaiting past it, so cleanup CANNOT live
/// inline after `system.run().await`. The watchdog runs in its own task,
/// outside the abort scope, and polls every 500 ms.
///
/// Cleanup scope: this helper deletes only
///   - the label hash for each `member_id` (via `label_key_fn`),
///   - membership in the global `label_index_key` SET,
///   - membership in the per-window `grouping_index_key` SET.
///
/// It does NOT touch `ob:/bba:/snapshots:/trades:` — those are keyed by
/// the stable `base_slug` / `series` and are reused across rollovers
/// (the next window's first snapshot overwrites them). If the engine is
/// shutting down for good, leave the last snapshot in Redis for
/// post-mortem inspection.
pub(super) fn spawn_label_watchdog(
    redis: RedisHandle,
    control: SystemControl,
    label_index_key: &'static str,
    grouping_index_key: String,
    member_ids: Vec<String>,
    label_key_fn: impl Fn(&str) -> String + Send + 'static,
) {
    tokio::spawn(async move {
        loop {
            if control.is_shutdown() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        for id in &member_ids {
            redis.del(&label_key_fn(id)).await;
            redis.srem(label_index_key, id).await;
            redis.srem(&grouping_index_key, id).await;
        }
    });
}

/// Per-window forward-hook context, shared between snapshot and trade hooks.
///
/// The fields are Clone-friendly because each `on::<T, _>(closure)`
/// registration captures its own copy — closures are `'static` so they
/// can't borrow.
#[derive(Clone)]
pub(super) struct RollingHookCtx {
    pub bus: StreamEngineBus,
    pub redis: Option<RedisHandle>,
    /// `"polymarket"` or `"kalshi"`. Used for `set_orderbook` / `set_bba`
    /// key prefixes and for the ZMQ topic dispatch on the receiving side.
    pub exchange: &'static str,
    /// Stable, rollover-safe identifier for ZMQ topic + Redis storage key:
    /// `base_slug` for Polymarket, `series` for Kalshi. Routed onto
    /// `StreamEvent::Rolling{Snapshot,LastTradePrice}.base_slug`.
    pub grouping_slug: String,
    /// Per-window identifier stamped into payload `full_slug`:
    /// `{base_slug}-{window_start}` for Polymarket, the per-window
    /// `ticker` for Kalshi. Lets subscribers detect rollover from the
    /// payload without resubscribing.
    pub full_slug: String,
    /// Unix seconds of this window's start. The hook drops every frame
    /// with `now_secs() < window_start` as a defence-in-depth check
    /// against the WS bootstrap firing before the scheduler intended
    /// (rare). The scheduler now spawns each window AT the boundary
    /// rather than pre-spawning, so the lower-gate is rarely hit in
    /// practice — but it's a cheap safety net.
    pub window_start: u64,
    pub snapshot_cap: usize,
    pub trade_cap: usize,
}

/// Forward one snapshot: gate on boundary, stamp `full_slug`, publish onto
/// the bus, and mirror to Redis under the stable `(exchange, grouping_slug)`
/// keys. Callers must have already filtered out venue-specific drops
/// (e.g. Polymarket's DOWN-side suppression) — this fn just does the work.
fn forward_snapshot(ctx: &RollingHookCtx, snap: &OrderbookSnapshot) {
    if libs::time::now_secs() < ctx.window_start {
        return;
    }
    // Stamp `full_slug` so subscribers on the stable rolling topic
    // `polymarket:{base_slug}` know which window the frame belongs to.
    // `symbol` is left as the venue asset_id — never overwritten.
    let mut stamped = snap.clone();
    stamped.full_slug = Some(ctx.full_slug.clone());
    ctx.bus.publish(StreamEvent::RollingSnapshot {
        base_slug: ctx.grouping_slug.clone(),
        snap: stamped.clone(),
    });
    if let Some(r) = &ctx.redis {
        let exchange = ctx.exchange;
        let slug = ctx.grouping_slug.clone();
        let snap_cap = ctx.snapshot_cap;
        if let Ok(ob_bytes) = rmp_serde::to_vec_named(&stamped) {
            let r1 = r.clone();
            let s1 = slug.clone();
            let b1 = ob_bytes.clone();
            tokio::spawn(async move { r1.set_orderbook(exchange, &s1, &b1).await });
            let r2 = r.clone();
            let s2 = slug.clone();
            tokio::spawn(async move {
                r2.lpush_capped(&RedisKey::snapshots(exchange, &s2), &ob_bytes, snap_cap)
                    .await
            });
        }
        if let Ok(bba_bytes) = rmp_serde::to_vec_named(&BbaPayload::from(&stamped)) {
            let r1 = r.clone();
            let s1 = slug.clone();
            tokio::spawn(async move { r1.set_bba(exchange, &s1, &bba_bytes).await });
        }
    }
}

/// Forward one trade: same shape as `forward_snapshot`, list-only writes.
fn forward_trade(ctx: &RollingHookCtx, trade: &LastTradePrice) {
    if libs::time::now_secs() < ctx.window_start {
        return;
    }
    // See `forward_snapshot` — stamp `full_slug`, leave `symbol`
    // (= venue asset_id) untouched.
    let mut stamped = trade.clone();
    stamped.full_slug = Some(ctx.full_slug.clone());
    ctx.bus.publish(StreamEvent::RollingLastTradePrice {
        base_slug: ctx.grouping_slug.clone(),
        trade: stamped.clone(),
    });
    if let Some(r) = &ctx.redis {
        if let Ok(bytes) = rmp_serde::to_vec_named(&stamped) {
            let exchange = ctx.exchange;
            let slug = ctx.grouping_slug.clone();
            let trd_cap = ctx.trade_cap;
            let r1 = r.clone();
            tokio::spawn(async move {
                r1.lpush_capped(&RedisKey::trades(exchange, &slug), &bytes, trd_cap)
                    .await
            });
        }
    }
}

/// Register snapshot + trade forward hooks on a Kalshi per-window engine.
///
/// Kalshi has no up/down split — every frame from the connected ticker is
/// forwarded (subject to the `window_start` gate inside `forward_*`).
pub(super) fn register_kalshi_rolling_hooks(engine: &mut StreamEngine, ctx: RollingHookCtx) {
    let snap_ctx = ctx.clone();
    engine
        .hooks_mut()
        .on::<OrderbookSnapshot, _>(move |s| forward_snapshot(&snap_ctx, s));
    let trade_ctx = ctx;
    engine
        .hooks_mut()
        .on::<LastTradePrice, _>(move |t| forward_trade(&trade_ctx, t));
}

/// Register snapshot + trade forward hooks on a Polymarket per-window engine.
///
/// Polymarket Up/Down: both asset_ids' WS frames arrive in this engine
/// (needed for full materialisation), but ONLY the UP side is forwarded
/// onto the bus + Redis. The DOWN token is a price mirror of UP
/// (buying YES at p = selling NO at 1-p), so a single time-series under
/// the base_slug captures the market completely.
///
/// `is_up_by_asset` maps each subscribed `asset_id` → whether it's the UP
/// token. Missing entries are treated as not-UP (defensive).
pub(super) fn register_polymarket_rolling_hooks(
    engine: &mut StreamEngine,
    ctx: RollingHookCtx,
    is_up_by_asset: HashMap<String, bool>,
) {
    let snap_ctx = ctx.clone();
    let snap_is_up = is_up_by_asset.clone();
    engine
        .hooks_mut()
        .on::<OrderbookSnapshot, _>(move |s| {
            if !snap_is_up.get(&s.symbol).copied().unwrap_or(false) {
                return;
            }
            forward_snapshot(&snap_ctx, s);
        });
    let trade_ctx = ctx;
    engine
        .hooks_mut()
        .on::<LastTradePrice, _>(move |t| {
            if !is_up_by_asset.get(&t.symbol).copied().unwrap_or(false) {
                return;
            }
            forward_trade(&trade_ctx, t);
        });
}
