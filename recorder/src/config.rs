use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum RotationPolicy {
    /// Create a new file per UTC day: `{base}/{exchange}/{symbol}/{YYYY-MM-DD}.mpack`
    Daily,
    /// Never rotate — single file per symbol. Useful for short tests.
    None,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Root directory for data files. Default: `"./data"`
    pub base_path: PathBuf,
    /// Number of depth levels (bids + asks) to store per snapshot. Default: `10`
    pub depth: usize,
    /// How often (ms) to flush `BufWriter`s to the OS. Default: `1000`
    pub flush_interval_ms: u64,
    /// File rotation policy. Default: `Daily`
    pub rotation: RotationPolicy,
    /// zstd compression level (1–22). `None` disables compression.
    /// Files are compressed after rotation, producing `{date}.mpack.zst`.
    pub zstd_level: Option<i32>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("./data"),
            depth: 10,
            flush_interval_ms: 1000,
            rotation: RotationPolicy::Daily,
            zstd_level: None,
        }
    }
}
