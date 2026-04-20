use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub enabled: bool,
    pub url: String,
    /// Max entries kept in each capped event list (fills, orders, log).
    pub event_list_cap: usize,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "redis://127.0.0.1:6379".to_string(),
            event_list_cap: 5000,
        }
    }
}
