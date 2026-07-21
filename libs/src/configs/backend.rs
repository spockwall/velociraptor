use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    pub port: u16,
    /// Root directory whose immediate subfolders the monitor reports
    /// directory (du-style) usage for. On the server this is the data volume
    /// (`/data`); on a dev box without it the monitor's `data_usage` is just
    /// empty (no error).
    pub data_dir: String,
    /// Polymarket auto-redeem watchdog (see `backend::routes::redeem`).
    pub redeem_watch: RedeemWatchConfig,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            data_dir: "/data".to_string(),
            redeem_watch: RedeemWatchConfig::default(),
        }
    }
}

/// Polymarket auto-redeem watchdog. Polymarket normally auto-redeems winning
/// positions shortly after resolution; when that pipeline breaks, winning
/// shares sit unconverted. The backend polls the public data-api for
/// `redeemable` positions and alerts when one stays unredeemed too long.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedeemWatchConfig {
    pub enabled: bool,
    /// Seconds between data-api polls.
    pub poll_secs: u64,
    /// A winning position still redeemable after this many seconds is
    /// considered stuck (auto-redeem presumed broken) and raises an ERROR log.
    pub alert_after_secs: u64,
    /// Ignore redeemable positions worth less than this (USD). Losing
    /// positions are also `redeemable` upstream but worth $0 and never
    /// auto-redeemed — they must not trip the alarm.
    pub min_value_usd: f64,
    /// Extra proxy wallets to watch. The wallet from the Polymarket
    /// credentials file (`funder`, falling back to `address`) is added
    /// automatically when present.
    pub wallets: Vec<String>,
}

impl Default for RedeemWatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_secs: 60,
            alert_after_secs: 900,
            min_value_usd: 1.0,
            wallets: Vec::new(),
        }
    }
}
