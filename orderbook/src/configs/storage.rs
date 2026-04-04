use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub enabled: bool,
    pub base_path: String,
    pub depth: usize,
    pub flush_interval: u64,
    /// `"daily"` rotates files at midnight UTC. `"none"` writes a single file per symbol.
    pub rotation: String,
    /// zstd compression level (1–22). `0` disables compression.
    /// After daily rotation, the previous file is compressed to `.mpack.zst`.
    pub zstd_level: u8,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_path: "./data".into(),
            depth: 10,
            flush_interval: 1000,
            rotation: "daily".into(),
            zstd_level: 0,
        }
    }
}
