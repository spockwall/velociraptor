//! Polymarket token resolution — calls the Gamma REST API at startup to convert
//! slug-based config entries into `clobTokenIds` for WebSocket subscription,
//! and spawns a rotator task that re-resolves tokens as rolling windows expire.

use crate::configs::exchanges::PolymarketMarket;
use libs::time::now_secs;
use tracing::{info, warn};

const GAMMA_BASE: &str = "https://gamma-api.polymarket.com/markets/slug";

// ── Slug computation ──────────────────────────────────────────────────────────

/// Compute full slugs for a market entry.
/// - Static market (`interval_secs == 0`): returns the slug as-is.
/// - Rolling market: computes the current window-end timestamp and appends it,
///   plus `prefetch_windows` future windows.
pub fn build_slug(base_slug: &str, interval_secs: u64) -> String {
    if interval_secs == 0 {
        return base_slug.to_string();
    }
    let now = now_secs();
    // Current window start = floor(now / interval) * interval.
    let base_ts = (now / interval_secs) * interval_secs;
    format!("{base_slug}-{base_ts}") // slug 
}

// ── REST resolution ───────────────────────────────────────────────────────────

/// Fetch `clobTokenIds` for a single slug. Returns an empty vec on 404.
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

// ── Startup resolution ────────────────────────────────────────────────────────

/// Call the Gamma REST API to resolve token IDs for all enabled polymarket entries.
/// Runs synchronously at startup — not on the hot path.
pub fn resolve_assets(markets: &[PolymarketMarket]) -> Vec<String> {
    resolve_assets_with_labels(markets)
        .into_iter()
        .map(|(id, _, _, _)| id)
        .collect()
}

/// Like `resolve_assets` but also returns routing info for each token ID.
/// Returns `(token_id, base_slug, full_slug, is_up)` tuples.
///   - `base_slug`  — the original slug from config (no timestamp), used as the panel key
///   - `full_slug`  — the fully-resolved slug with timestamp (e.g. "btc-updown-5m-1775308500")
///   - `is_up`      — true = Up/Yes outcome, false = Down/No outcome
pub fn resolve_assets_with_labels(
    markets: &[PolymarketMarket],
) -> Vec<(String, String, String, bool)> {
    let Some(client) = build_rest_client() else {
        return vec![];
    };

    let mut assets = Vec::new();
    for m in markets.iter().filter(|m| m.enabled && !m.slug.is_empty()) {
        let slug = build_slug(&m.slug, m.interval_secs);
        let ids = fetch_token_ids(&client, &slug);
        if !ids.is_empty() {
            info!(slug, tokens = ids.len(), "Resolved Polymarket tokens");
            // Index 0 = Up/Yes, index 1 = Down/No
            for (i, id) in ids.into_iter().enumerate() {
                assets.push((id, m.slug.clone(), slug.clone(), i == 0));
            }
        }
    }
    assets
}
