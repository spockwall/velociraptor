use crate::types::endpoints::kalshi;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use chrono_tz::US::Eastern;

// ── Ticker computation ────────────────────────────────────────────────────────

// Kalshi tickers encode the window **start** time in US Eastern Time (EDT/EST,
// DST-aware). e.g. a window starting at 2026-04-15 14:45 UTC = 10:45 EDT
// → segment "26APR151045". We use `chrono-tz` for correct DST handling.

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Convert a UTC datetime to the Kalshi ticker datetime segment (Eastern time).
/// e.g. `2026-04-15T14:45:00Z` (= 10:45 EDT) → `"26APR151045"`
pub fn format_ticker_dt(utc: DateTime<Utc>) -> String {
    let et = utc.with_timezone(&Eastern);
    format!(
        "{:02}{}{:02}{:02}{:02}",
        et.year() % 100,
        MONTHS[(et.month() as usize) - 1],
        et.day(),
        et.hour(),
        et.minute(),
    )
}

/// Compute the UTC start time of the current 15-min window.
/// Windows are aligned on :00, :15, :30, :45 of each UTC hour.
/// e.g. if now = 10:53, returns 10:45.
pub fn current_window_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let min = now.minute();
    let boundary = (min / 15) * 15;
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), now.hour(), boundary, 0)
        .unwrap()
}

/// Compute the UTC close time of the current 15-min window.
pub fn current_window_close(now: DateTime<Utc>) -> DateTime<Utc> {
    current_window_start(now) + Duration::minutes(15)
}

/// Build the Kalshi **event** ticker: `"KXBTC15M-26APR160700"`.
///
/// Kalshi's 15-min crypto markets have three dash-separated segments:
/// `<series>-<YYMONDDHHMM in ET>-<strike>`. The first two segments form the
/// *event* ticker; the third strike suffix is assigned per-window by Kalshi
/// (it's not a fixed constant — the "target price" rounds differently each
/// window) and must be resolved via REST. See [`resolve_market_ticker`].
pub fn build_event_ticker(series: &str, close_utc: DateTime<Utc>) -> String {
    format!("{}-{}", series, format_ticker_dt(close_utc))
}

/// Resolve the full market ticker (e.g. `"KXBTC15M-26APR160700-00"`) from an
/// event ticker by querying Kalshi's public `/markets` endpoint.
///
/// Kalshi assigns the strike suffix dynamically per window based on the
/// prevailing index price, so it can't be computed locally. This hits the
/// unauthenticated REST endpoint and returns the first (and only) market for
/// the event.
pub async fn resolve_market_ticker(
    event_ticker: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "{}/markets?event_ticker={}",
        kalshi::REST_BASE,
        event_ticker
    );
    let resp: serde_json::Value = reqwest::get(&url).await?.error_for_status()?.json().await?;
    resp.get("markets")
        .and_then(|m| m.as_array())
        .and_then(|a| a.first())
        .and_then(|m| m.get("ticker"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no market for event {event_ticker}").into())
}
