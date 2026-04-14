use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KalshiMarketConfig {
    pub enable: bool,
    pub series: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KalshiConfig {
    /// Listed under `market` in the yaml (array of market entries).
    pub market: Vec<KalshiMarketConfig>,
}
