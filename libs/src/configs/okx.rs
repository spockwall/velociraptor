use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct OkxConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}
