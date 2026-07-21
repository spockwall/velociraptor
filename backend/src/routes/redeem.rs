//! Polymarket auto-redeem watchdog.
//!
//! Polymarket normally auto-redeems winning positions shortly after a market
//! resolves (their relayer converts the winning shares to USDC). That
//! pipeline occasionally breaks, leaving winning shares sitting unredeemed.
//! This module polls the public data-api for `redeemable` positions on the
//! configured wallet(s) and raises an alert when one stays unredeemed past
//! `alert_after_secs`.
//!
//!   - [`watch_loop`] — background task (spawned by `bin/backend.rs`), one
//!     data-api poll per wallet every `poll_secs`. Writes the latest status
//!     (JSON) to the Redis key [`Redeem::STATUS`] and ERROR-logs stuck
//!     positions so they surface on the frontend Logs page via the error-log
//!     tailer.
//!   - `GET /api/pm/redeem-status` — returns the latest status blob.
//!
//! Detection rules:
//!   - Only positions worth at least `min_value_usd` count. A resolved LOSING
//!     position is also `redeemable: true` upstream, but it is worth $0 and
//!     auto-redeem never touches it — it must not trip the alarm.
//!   - First-seen times are persisted in a per-wallet Redis hash
//!     ([`Redeem::first_seen`]) so a backend restart doesn't reset the stuck
//!     timers. An already-stuck position re-alerts once after a restart.
//!   - A position leaving the redeemable set was redeemed (auto or manual):
//!     its timer is dropped and the recovery is INFO-logged.
//!   - A data-api fetch failure skips the reconcile for that wallet (timers
//!     are kept, nothing is cleared) and is reported in the status blob.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::response::Json;
use libs::configs::RedeemWatchConfig;
use libs::redis_client::keys::Redeem;
use libs::redis_client::RedisHandle;
use libs::time::now_secs;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, warn};

use crate::error::ApiError;
use crate::state::AppState;

use super::pmexplorer::DATA_API;

/// The slice of a data-api position entry the watcher cares about.
/// Unknown fields are ignored; everything defaults so upstream schema
/// drift degrades gracefully instead of dropping the whole poll.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct DataApiPosition {
    asset: String,
    condition_id: String,
    size: f64,
    cur_price: f64,
    current_value: f64,
    title: String,
    slug: String,
    outcome: String,
    end_date: String,
    negative_risk: bool,
}

impl Default for DataApiPosition {
    fn default() -> Self {
        Self {
            asset: String::new(),
            condition_id: String::new(),
            size: 0.0,
            cur_price: 0.0,
            current_value: 0.0,
            title: String::new(),
            slug: String::new(),
            outcome: String::new(),
            end_date: String::new(),
            negative_risk: false,
        }
    }
}

/// Background watcher. Runs for the life of the process.
pub async fn watch_loop(
    redis: RedisHandle,
    http: reqwest::Client,
    cfg: RedeemWatchConfig,
    wallets: Vec<String>,
) {
    info!(
        wallets = ?wallets,
        poll_secs = cfg.poll_secs,
        alert_after_secs = cfg.alert_after_secs,
        min_value_usd = cfg.min_value_usd,
        "redeem watcher: started"
    );

    // Assets already ERROR-logged as stuck, so each one alerts once per
    // stuck episode (per process) instead of every poll.
    let mut alerted: HashSet<String> = HashSet::new();

    let mut tick = tokio::time::interval(Duration::from_secs(cfg.poll_secs.max(10)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;

        let mut reports = Vec::with_capacity(wallets.len());
        for w in &wallets {
            reports.push(check_wallet(&redis, &http, &cfg, w, &mut alerted).await);
        }

        let status = json!({
            "ts": now_secs(),
            "poll_secs": cfg.poll_secs,
            "alert_after_secs": cfg.alert_after_secs,
            "min_value_usd": cfg.min_value_usd,
            "wallets": reports,
        });
        let _: Result<(), redis::RedisError> = redis::cmd("SET")
            .arg(Redeem::STATUS)
            .arg(status.to_string())
            .query_async(&mut redis.raw())
            .await;
    }
}

/// One poll for one wallet: fetch redeemable positions, advance/clear the
/// persisted first-seen timers, alert on threshold crossings, and return the
/// wallet's slice of the status blob.
async fn check_wallet(
    redis: &RedisHandle,
    http: &reqwest::Client,
    cfg: &RedeemWatchConfig,
    wallet: &str,
    alerted: &mut HashSet<String>,
) -> serde_json::Value {
    let url =
        format!("{DATA_API}/positions?user={wallet}&redeemable=true&sizeThreshold=0&limit=500");
    let positions: Vec<DataApiPosition> = match fetch_positions(http, &url).await {
        Ok(p) => p,
        Err(e) => {
            warn!(wallet, "redeem watcher: positions fetch failed: {e}");
            return json!({
                "wallet": wallet,
                "ok": false,
                "error": e,
            });
        }
    };

    let now = now_secs();
    let first_seen_key = Redeem::first_seen(wallet);
    let seen = redis.hgetall(&first_seen_key).await;

    // Winning (valuable) redeemable positions — the ones auto-redeem should
    // have handled. Losing/dust positions are counted but not timed.
    let watched: Vec<&DataApiPosition> = positions
        .iter()
        .filter(|p| !p.asset.is_empty() && p.current_value >= cfg.min_value_usd)
        .collect();

    let mut new_fields: Vec<(String, String)> = Vec::new();
    let mut entries = Vec::with_capacity(watched.len());
    let mut stuck_count = 0usize;
    let mut stuck_value_usd = 0.0f64;

    for p in &watched {
        let first_seen = match seen.get(&p.asset).and_then(|v| v.parse::<u64>().ok()) {
            Some(t) => t.min(now),
            None => {
                new_fields.push((p.asset.clone(), now.to_string()));
                now
            }
        };
        let waiting_secs = now - first_seen;
        let stuck = waiting_secs >= cfg.alert_after_secs;
        if stuck {
            stuck_count += 1;
            stuck_value_usd += p.current_value;
            if alerted.insert(p.asset.clone()) {
                error!(
                    wallet,
                    title = %p.title,
                    outcome = %p.outcome,
                    value_usd = p.current_value,
                    waiting_secs,
                    condition_id = %p.condition_id,
                    "redeem watcher: auto-redeem appears BROKEN — winning position still unredeemed"
                );
            }
        }
        entries.push(json!({
            "asset": p.asset,
            "condition_id": p.condition_id,
            "title": p.title,
            "slug": p.slug,
            "outcome": p.outcome,
            "size": p.size,
            "cur_price": p.cur_price,
            "current_value": p.current_value,
            "end_date": p.end_date,
            "negative_risk": p.negative_risk,
            "first_seen": first_seen,
            "waiting_secs": waiting_secs,
            "stuck": stuck,
        }));
    }

    if !new_fields.is_empty() {
        let pairs: Vec<(&str, &str)> = new_fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        redis.hset_multi(&first_seen_key, &pairs).await;
    }

    // Anything timed before but no longer in the watched set was redeemed
    // (auto-redeem caught up, or a manual redeem) — drop its timer.
    let current: HashSet<&str> = watched.iter().map(|p| p.asset.as_str()).collect();
    let cleared: Vec<&str> = seen
        .keys()
        .filter(|a| !current.contains(a.as_str()))
        .map(|a| a.as_str())
        .collect();
    if !cleared.is_empty() {
        for a in &cleared {
            let waited = seen
                .get(*a)
                .and_then(|v| v.parse::<u64>().ok())
                .map(|t| now.saturating_sub(t))
                .unwrap_or(0);
            let was_stuck = alerted.remove(*a);
            info!(
                wallet,
                asset = a,
                waited_secs = waited,
                was_stuck,
                "redeem watcher: position redeemed / cleared"
            );
        }
        redis.hdel(&first_seen_key, &cleared).await;
    }

    json!({
        "wallet": wallet,
        "ok": stuck_count == 0,
        "error": serde_json::Value::Null,
        "redeemable_total": positions.len(),
        "watched": entries.len(),
        "stuck_count": stuck_count,
        "stuck_value_usd": stuck_value_usd,
        "positions": entries,
    })
}

async fn fetch_positions(
    http: &reqwest::Client,
    url: &str,
) -> Result<Vec<DataApiPosition>, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| format!("json: {e}"))
}

/// `GET /api/pm/redeem-status` — the watcher's latest status blob. 404 until
/// the watcher has completed its first poll (or when it is disabled).
pub(crate) async fn get_redeem_status(
    State(s): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let bytes = s.redis.get_raw(Redeem::STATUS).await.ok_or_else(|| {
        ApiError::NotFound("no redeem status yet — watcher disabled or not polled".into())
    })?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::Decode(format!("redeem status json: {e}")))?;
    Ok(Json(v))
}
