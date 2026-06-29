use chrono::{DateTime, TimeZone, Utc};

pub fn parse_timestamp_ms(ts: &str) -> DateTime<Utc> {
    ts.parse::<i64>()
        .ok()
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .unwrap_or_else(Utc::now)
}

pub fn current_date() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
}

pub fn now_ns() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

/// Convert a `DateTime<Utc>` to Unix nanoseconds (`0` if out of range).
pub fn dt_to_ns(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_nanos_opt().unwrap_or(0)
}

/// Parse a Unix-millisecond string (e.g. `"1750428146322"`) to Unix
/// nanoseconds. Returns `0` when the string is empty or not an integer —
/// callers use `0` to mean "no exchange timestamp available".
pub fn parse_ms_to_ns(ts: &str) -> i64 {
    ts.parse::<i64>().ok().map(|ms| ms * 1_000_000).unwrap_or(0)
}

/// Parse an RFC3339 timestamp string to Unix nanoseconds. Returns `0` when
/// the string is empty or unparseable.
pub fn parse_rfc3339_to_ns(ts: &str) -> i64 {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt_to_ns(dt.with_timezone(&Utc)))
        .unwrap_or(0)
}
