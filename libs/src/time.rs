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
