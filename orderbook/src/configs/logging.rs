use serde::Deserialize;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub json: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            json: false,
        }
    }
}

pub fn init_logging(filter: &str, json: bool) {
    let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }
}
