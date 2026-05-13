use serde::Deserialize;

/// Logging config — controls the shared `libs::logging::init_logging` setup.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// env-filter string (e.g. "info", "info,libs=debug").
    pub level: String,
    /// Emit JSON instead of human-readable text.
    pub json: bool,
    /// Root directory for the daily-rotating files.
    /// Files land at `{dir}/{service}/log.YYYY-MM-DD` and `…/error.log.YYYY-MM-DD`.
    pub dir: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            json: false,
            dir: "/app/syslog".to_string(),
        }
    }
}
