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

/// Wall-clock ns since the UNIX epoch as `u64` (clamps the pre-1970 case to 0).
/// Used for latency-tracing stamps (`OrderMeta`, `OrderbookUpdate::recv_ns`)
/// where a `u64` is wanted.
pub fn now_ns_u64() -> u64 {
    now_ns().max(0) as u64
}
