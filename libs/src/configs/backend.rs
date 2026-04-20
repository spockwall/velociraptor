use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    pub port: u16,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self { port: 3000 }
    }
}
