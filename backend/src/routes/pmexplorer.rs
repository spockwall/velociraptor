//! Polymarket Explorer — public per-user + market data proxy.
//!
//! Powers the frontend `/pm-explorer` page, which lets you inspect ANY
//! Polymarket account by wallet address (the same data the public
//! polymarket.com profile renders). All upstream surfaces here are public /
//! unauthenticated and keyed by the user's **proxy wallet address**. We proxy
//! them server-side so the browser keeps talking only to `/api` (no CORS) and
//! so we can cache the heavier, slow-changing responses in Redis.
//!
//! Upstream surfaces (verified live):
//!   - `https://data-api.polymarket.com/positions?user=<wallet>`
//!   - `https://data-api.polymarket.com/activity?user=<wallet>[&type=TRADE]`
//!   - `https://data-api.polymarket.com/value?user=<wallet>`   → `[{user,value}]`
//!   - `https://data-api.polymarket.com/holders?market=<conditionId>`
//!   - `https://lb-api.polymarket.com/volume|profit?window=<w>&limit=<n>`
//!   - `https://gamma-api.polymarket.com/markets/slug/<slug>`  (liquidity/volume)
//!
//! Routes:
//!   - `/api/pm/positions/:wallet`
//!   - `/api/pm/activity/:wallet`        (`?type=`, `?limit=`)
//!   - `/api/pm/value/:wallet`
//!   - `/api/pm/holders/:condition_id`
//!   - `/api/pm/leaderboard`             (`?metric=volume|profit&window=&limit=`)
//!   - `/api/pm/market/:slug`
//!   - `/api/pm/resolve/:id`             (address → echo; username → best-effort)

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

const DATA_API: &str = "https://data-api.polymarket.com";
const LB_API: &str = "https://lb-api.polymarket.com";
const GAMMA_API: &str = "https://gamma-api.polymarket.com";
const SITE_API: &str = "https://polymarket.com/api";
const PNL_API: &str = "https://user-pnl-api.polymarket.com";

// ── helpers ──────────────────────────────────────────────────────────────────

/// True for a `0x`-prefixed 40-hex-char EVM address.
fn is_address(s: &str) -> bool {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    matches!(hex, Some(h) if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Validate + normalise a wallet path param, or 400.
fn require_address(raw: &str) -> Result<String, ApiError> {
    let w = raw.trim();
    if is_address(w) {
        Ok(w.to_lowercase())
    } else {
        Err(ApiError::BadRequest(format!(
            "'{raw}' is not a 0x wallet address"
        )))
    }
}

/// Minimal percent-encoder for a single query value (the workspace has no
/// `urlencoding` dep). Encodes everything outside the RFC3986 unreserved set.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn fetch_json(
    http: &reqwest::Client,
    url: &str,
    what: &str,
) -> Result<serde_json::Value, ApiError> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::Network(format!("{what}: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::Network(format!("{what}: HTTP {}", resp.status())));
    }
    resp.json()
        .await
        .map_err(|e| ApiError::Decode(format!("{what} json: {e}")))
}

/// Cache-first fetch: return the cached JSON if present, else fetch via
/// `build` and store it with `ttl` seconds (SET EX, no NX needed — last writer
/// wins is fine for read-only snapshots). Mirrors the pattern in `spot.rs`.
async fn cached<F>(
    s: &AppState,
    key: &str,
    ttl: u64,
    build: F,
) -> Result<serde_json::Value, ApiError>
where
    F: std::future::Future<Output = Result<serde_json::Value, ApiError>>,
{
    if let Some(bytes) = s.redis.get_raw(key).await {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                return Ok(v);
            }
        }
    }
    let v = build.await?;
    let _: Result<(), redis::RedisError> = redis::cmd("SET")
        .arg(key)
        .arg(v.to_string())
        .arg("EX")
        .arg(ttl)
        .query_async(&mut s.redis.raw())
        .await;
    Ok(v)
}

// ── query params ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct ActivityQuery {
    /// Optional event-type filter (e.g. `TRADE`). Forwarded as `type=`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub limit: Option<u32>,
}

/// Shared list knobs for `/closed-positions` and `/trades`.
#[derive(Deserialize)]
pub(crate) struct ListQuery {
    pub limit: Option<u32>,
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
    /// `/trades` only: restrict to taker-side fills (default true).
    #[serde(rename = "takerOnly")]
    pub taker_only: Option<bool>,
}

#[derive(Deserialize)]
pub(crate) struct PnlQuery {
    /// `1d`, `1w`, `1m`, or `all` (default).
    pub interval: Option<String>,
    /// Point granularity: `1h` or `1d` (default).
    pub fidelity: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LeaderboardQuery {
    /// `volume` (default) or `profit`.
    pub metric: Option<String>,
    /// `all` (default), `7d`, `30d`.
    pub window: Option<String>,
    pub limit: Option<u32>,
}

// ── handlers ─────────────────────────────────────────────────────────────────

pub(crate) async fn get_positions(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let url = format!("{DATA_API}/positions?user={w}&sizeThreshold=0");
    Ok(Json(fetch_json(&s.gamma, &url, "positions").await?))
}

/// Resolved/closed positions — dedicated data-api endpoint carrying
/// per-market `realizedPnl` + `timestamp`. Cleaner than filtering `/positions`
/// by `redeemable`.
pub(crate) async fn get_closed_positions(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let limit = q.limit.unwrap_or(100).min(500);
    let sort_by = q.sort_by.as_deref().unwrap_or("REALIZEDPNL");
    let dir = q.sort_direction.as_deref().unwrap_or("DESC");
    let url = format!(
        "{DATA_API}/closed-positions?user={w}&limit={limit}&sortBy={}&sortDirection={}",
        enc(sort_by),
        enc(dir),
    );
    Ok(Json(fetch_json(&s.gamma, &url, "closed-positions").await?))
}

/// On-chain trades for a wallet. `takerOnly=true` (default) returns only the
/// trades where this wallet was the taker — i.e. the actual order fills, not
/// the maker side of someone else's trade.
pub(crate) async fn get_trades(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let limit = q.limit.unwrap_or(100).min(500);
    let taker = q.taker_only.unwrap_or(true);
    let url = format!("{DATA_API}/trades?user={w}&limit={limit}&takerOnly={taker}");
    Ok(Json(fetch_json(&s.gamma, &url, "trades").await?))
}

pub(crate) async fn get_activity(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let mut url = format!("{DATA_API}/activity?user={w}");
    if let Some(t) = q.kind.as_deref().filter(|t| !t.is_empty()) {
        url.push_str(&format!("&type={}", enc(t)));
    }
    url.push_str(&format!("&limit={}", q.limit.unwrap_or(100).min(500)));
    Ok(Json(fetch_json(&s.gamma, &url, "activity").await?))
}

pub(crate) async fn get_value(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let url = format!("{DATA_API}/value?user={w}");
    // Upstream returns `[{user, value}]`; flatten so the UI gets a plain object.
    let raw = fetch_json(&s.gamma, &url, "value").await?;
    let value = raw
        .as_array()
        .and_then(|a| a.first())
        .and_then(|o| o.get("value"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Ok(Json(serde_json::json!({ "user": w, "value": value })))
}

/// P&L time-series — the data behind the profile-page chart. Each point is
/// `{ t: unix_secs, p: cumulative_pnl_usd }`; the last point is the current
/// total P&L (matches lb-api `/profit`).
pub(crate) async fn get_pnl(
    State(s): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<PnlQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = require_address(&wallet)?;
    let interval = match q.interval.as_deref() {
        Some("1d") => "1d",
        Some("1w") => "1w",
        Some("1m") => "1m",
        _ => "all",
    };
    let fidelity = match q.fidelity.as_deref() {
        Some("1h") => "1h",
        _ => "1d",
    };
    let url = format!("{PNL_API}/user-pnl?user_address={w}&interval={interval}&fidelity={fidelity}");
    Ok(Json(fetch_json(&s.gamma, &url, "pnl").await?))
}

pub(crate) async fn get_holders(
    State(s): State<Arc<AppState>>,
    Path(condition_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cid = condition_id.trim();
    if !cid.starts_with("0x") {
        return Err(ApiError::BadRequest(format!(
            "'{condition_id}' is not a 0x condition id"
        )));
    }
    let cid = cid.to_string();
    let key = format!("pm:holders:{cid}");
    let url = format!("{DATA_API}/holders?market={cid}");
    let v = cached(&s, &key, 60, fetch_json(&s.gamma, &url, "holders")).await?;
    Ok(Json(v))
}

pub(crate) async fn get_leaderboard(
    State(s): State<Arc<AppState>>,
    Query(q): Query<LeaderboardQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let metric = match q.metric.as_deref() {
        Some("profit") => "profit",
        _ => "volume",
    };
    let window = match q.window.as_deref() {
        Some("7d") => "7d",
        Some("30d") => "30d",
        _ => "all",
    };
    let limit = q.limit.unwrap_or(20).min(100);
    let key = format!("pm:lb:{metric}:{window}:{limit}");
    let url = format!("{LB_API}/{metric}?window={window}&limit={limit}");
    let v = cached(&s, &key, 60, fetch_json(&s.gamma, &url, "leaderboard")).await?;
    Ok(Json(v))
}

pub(crate) async fn get_market(
    State(s): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let slug = slug.trim().to_string();
    if slug.is_empty() {
        return Err(ApiError::BadRequest("empty market slug".into()));
    }
    let key = format!("pm:market:{slug}");
    let url = format!("{GAMMA_API}/markets/slug/{}", enc(&slug));
    let v = cached(&s, &key, 30, fetch_json(&s.gamma, &url, "market")).await?;
    Ok(Json(v))
}

/// Resolve an arbitrary `:id` to the **proxy wallet** the data-api keys on.
///   - `0x…40hex`     → looked up via the public profile API. The address in a
///     `polymarket.com/@…` profile URL is usually the user's EOA / login
///     address, while positions/activity/value are stored under a *different*
///     proxy (Gnosis Safe) wallet. We map EOA→proxy here so pasting either one
///     works. If the profile lookup yields nothing, we fall back to using the
///     address verbatim. `source` is `"proxy"` or `"address"`.
///   - anything else  → treated as a username; best-effort match against the
///     volume leaderboard's `name`/`pseudonym` fields. No reliable public
///     handle→wallet endpoint exists, so this is intentionally limited.
pub(crate) async fn resolve(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = id.trim().trim_start_matches('@').to_string();
    if is_address(&id) {
        // Map EOA → proxy via the public profile endpoint. Best-effort: any
        // failure (network, no profile) falls back to the address as-is.
        let url = format!("{SITE_API}/profile/userData?address={}", id);
        if let Ok(body) = fetch_json(&s.gamma, &url, "profile userData").await {
            if let Some(proxy) = body
                .get("proxyWallet")
                .and_then(|v| v.as_str())
                .filter(|p| is_address(p))
            {
                return Ok(Json(serde_json::json!({
                    "wallet": proxy.to_lowercase(),
                    "source": "proxy",
                    "input": id.to_lowercase(),
                    "name": body.get("name").and_then(|v| v.as_str()),
                    "pseudonym": body.get("pseudonym").and_then(|v| v.as_str()),
                })));
            }
        }
        return Ok(Json(serde_json::json!({
            "wallet": id.to_lowercase(),
            "source": "address",
        })));
    }
    // Best-effort: scan a generous slice of the all-time volume leaderboard for
    // a case-insensitive name/pseudonym hit.
    let url = format!("{LB_API}/volume?window=all&limit=500");
    let body = fetch_json(&s.gamma, &url, "resolve leaderboard").await?;
    if let Some(arr) = body.as_array() {
        for e in arr {
            let hit = ["name", "pseudonym"]
                .iter()
                .filter_map(|k| e.get(*k).and_then(|v| v.as_str()))
                .any(|s| s.eq_ignore_ascii_case(&id));
            if hit {
                if let Some(w) = e.get("proxyWallet").and_then(|v| v.as_str()) {
                    return Ok(Json(serde_json::json!({
                        "wallet": w.to_lowercase(),
                        "source": "leaderboard",
                    })));
                }
            }
        }
    }
    Err(ApiError::NotFound(format!(
        "could not resolve '{id}' to a wallet — paste a 0x address"
    )))
}
