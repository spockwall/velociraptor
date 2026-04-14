use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub enabled: bool,
    pub base_path: String,
    pub depth: usize,
    pub flush_interval: u64,
    pub rotation: String,
    pub zstd_level: u8,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_path: "./data".to_string(),
            depth: 20,
            flush_interval: 1000,
            rotation: "daily".to_string(),
            zstd_level: 0,
        }
    }
}
