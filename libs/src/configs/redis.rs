use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub enabled: bool,
    pub url: String,
    /// Max entries kept in `snapshots:{exchange}:{symbol}` lists.
    pub snapshot_cap: usize,
    /// Max entries kept in `trades:{exchange}:{symbol}` lists.
    pub trade_cap: usize,
    /// Max entries kept in each capped event list (fills, orders, log).
    pub event_list_cap: usize,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "redis://127.0.0.1:6379".to_string(),
            snapshot_cap: 100,
            trade_cap: 1000,
            event_list_cap: 5000,
        }
    }
}
