//! Polymarket token resolution — calls the Gamma REST API to convert
//! slug-based config entries into `clobTokenIds` for WebSocket
//! subscription.
//!
//! Surface:
//! - [`build_slug`] — slug → window-stamped slug.
//! - [`resolve_assets_with_labels`] — resolve a list of market configs
//!   synchronously (used by startup paths that need many markets).
//! - [`resolve_one_slug_async`] — resolve a single already-stamped slug
//!   async (used by the rolling-window scheduler to prefetch the next
//!   window's tokens while the current one is still running).
//!
//! Both resolver entry points return [`PolymarketLabeledAsset`] tuples
//! and share one private blocking core, [`resolve_one_blocking`], so
//! the index→`is_up` mapping (Polymarket Gamma returns
//! `["Yes", "No"]`, in that order) lives in exactly one place.

use libs::configs::PolymarketMarketConfig;
use libs::time::now_secs;
use tracing::{info, warn};

const GAMMA_BASE: &str = "https://gamma-api.polymarket.com/markets/slug";

/// One resolved Polymarket asset: `(token_id, base_slug, full_slug, is_up)`.
///   - `base_slug` — the original slug from config (no timestamp)
///   - `full_slug` — the fully-resolved slug with timestamp
///   - `is_up`     — true = Up/Yes outcome, false = Down/No outcome
pub type PolymarketLabeledAsset = (String, String, String, bool);

// ── Slug computation ──────────────────────────────────────────────────────────

/// Compute the full slug for a market entry.
///   - Static market (`interval_secs == 0`): returns the slug as-is.
///   - Rolling market: appends the current window's start timestamp.
pub fn build_slug(base_slug: &str, interval_secs: u64) -> String {
    if interval_secs == 0 {
        return base_slug.to_string();
    }
    let now = now_secs();
    let base_ts = (now / interval_secs) * interval_secs;
    format!("{base_slug}-{base_ts}")
}

// ── REST helpers (blocking) ───────────────────────────────────────────────────

fn build_rest_client() -> Option<reqwest::blocking::Client> {
    match reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => Some(c),
        Err(e) => {
            warn!("Failed to build HTTP client for Polymarket: {e}");
            None
        }
    }
}

/// Fetch `clobTokenIds` for a single slug. Returns an empty vec on 404
/// or any other failure (callers treat empty as "not open yet, retry").
fn fetch_token_ids(client: &reqwest::blocking::Client, slug: &str) -> Vec<String> {
    let url = format!("{GAMMA_BASE}/{slug}");
    let result = client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json::<serde_json::Value>());

    match result {
        Ok(body) => {
            let raw = &body["clobTokenIds"];
            let ids: Vec<String> = if raw.is_array() {
                serde_json::from_value(raw.clone()).unwrap_or_default()
            } else {
                serde_json::from_str(raw.as_str().unwrap_or("[]")).unwrap_or_default()
            };
            if ids.is_empty() {
                warn!(slug, "Polymarket slug returned no token IDs");
            }
            ids
        }
        Err(e) if e.status().map(|s| s.as_u16()) == Some(404) => {
            warn!(slug, "Polymarket slug not found (market not open yet?)");
            vec![]
        }
        Err(e) => {
            warn!(slug, "Failed to fetch Polymarket tokens: {e}");
            vec![]
        }
    }
}

/// **Shared core.** Resolve one (`base_slug`, `full_slug`) pair into
/// labeled assets. Index 0 in Gamma's `clobTokenIds` is Yes/Up; index 1
/// is No/Down. All other resolver entry points are thin wrappers around
/// this function so the labelling lives in exactly one place.
///
/// Returns an empty vec on any failure (no tokens, REST error, 404).
fn resolve_one_blocking(
    client: &reqwest::blocking::Client,
    base_slug: &str,
    full_slug: &str,
) -> Vec<PolymarketLabeledAsset> {
    let ids = fetch_token_ids(client, full_slug);
    if ids.is_empty() {
        return Vec::new();
    }
    info!(
        slug = full_slug,
        tokens = ids.len(),
        "Resolved Polymarket tokens"
    );
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| (id, base_slug.to_string(), full_slug.to_string(), i == 0))
        .collect()
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Resolve token IDs for every enabled config entry. Synchronous —
/// blocks the calling thread on the HTTP round-trips. Use only during
/// process startup; the rolling-window scheduler uses
/// [`resolve_one_slug_async`] on the hot path.
///
/// Returns `(token_id, base_slug, full_slug, is_up)` tuples.
pub fn resolve_assets_with_labels(
    markets: &[PolymarketMarketConfig],
) -> Vec<PolymarketLabeledAsset> {
    let Some(client) = build_rest_client() else {
        return Vec::new();
    };
    let mut assets = Vec::new();
    for m in markets.iter().filter(|m| m.enabled && !m.slug.is_empty()) {
        let full_slug = build_slug(&m.slug, m.interval_secs);
        assets.extend(resolve_one_blocking(&client, &m.slug, &full_slug));
    }
    assets
}

/// Async wrapper that resolves a SINGLE already-stamped `full_slug`.
/// Used by the rolling-window scheduler to prefetch the next window's
/// tokens concurrently with its `sleep_until(win_end)`.
pub async fn resolve_one_slug_async(
    base_slug: String,
    full_slug: String,
) -> Vec<PolymarketLabeledAsset> {
    tokio::task::spawn_blocking(move || {
        let Some(client) = build_rest_client() else {
            return Vec::new();
        };
        resolve_one_blocking(&client, &base_slug, &full_slug)
    })
    .await
    .unwrap_or_default()
}
