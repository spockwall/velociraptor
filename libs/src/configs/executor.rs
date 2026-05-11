use serde::Deserialize;

/// Executor binary settings (read from the `executor:` section of the
/// top-level config YAML). All fields are optional — missing values fall back
/// to `Default`, which mirrors the executor binary's hard-coded defaults.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExecutorConfig {
    /// ZMQ ROUTER bind for incoming `OrderRequest`s.
    pub router_endpoint: String,
    /// HTTP `/metrics` listen address (Prometheus scrape target).
    pub metrics_addr: String,
    /// Audit log directory (one file per day).
    pub audit_dir: String,
    /// Max entries kept in the Redis audit stream.
    pub audit_stream_cap: usize,
    /// Polymarket env tag — `"prod"` or `"testnet"`.
    pub polymarket_env: String,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            router_endpoint: "tcp://*:5557".into(),
            metrics_addr: "127.0.0.1:5558".into(),
            audit_dir: "./data/executor".into(),
            audit_stream_cap: 100_000,
            polymarket_env: "prod".into(),
        }
    }
}
