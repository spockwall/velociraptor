//! Resolve a Polymarket market slug to its `conditionId` via the Gamma REST API.
//!
//! The user-channel subscription needs the `conditionId` (a 0x-prefixed hex
//! string) for each market, not the `clobTokenIds` that the public market
//! channel uses.

const GAMMA_BASE: &str = "https://gamma-api.polymarket.com/markets/slug";

/// Fetch `conditionId` for a fully-resolved Polymarket slug.
/// Returns `None` on 404, network failure, or missing field.
pub fn fetch_condition_id(client: &reqwest::blocking::Client, slug: &str) -> Option<String> {
    let url = format!("{GAMMA_BASE}/{slug}");
    let body: serde_json::Value = match client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[condition] GET {slug} failed: {e}");
            return None;
        }
    };

    body.get("conditionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Build a short-lived blocking HTTP client with a browser user-agent
/// (Gamma API rejects the default reqwest UA).
pub fn build_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()
}
