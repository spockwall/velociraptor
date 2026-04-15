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

/// Build a full market ticker: `"KXBTC15M-26APR151045-15"`.
///
/// Kalshi's 15-min crypto markets have three dash-separated segments:
/// `<series>-<YYMONDDHHMM in ET>-<strike>`. The datetime segment encodes the
/// window **start** time. The first two segments form the *event* ticker;
/// appending the strike yields the *market* ticker used in WS subscribe.
pub fn build_ticker(series: &str, start_utc: DateTime<Utc>) -> String {
    const STRIKE: &str = "15";
    format!("{}-{}-{}", series, format_ticker_dt(start_utc), STRIKE)
}
