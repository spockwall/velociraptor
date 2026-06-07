//! Shared application state passed to every handler.

use libs::redis_client::RedisHandle;

/// Resources every handler needs to do its job.
pub struct AppState {
    /// Connection-managed Redis handle. All read/write paths go through this.
    pub redis: RedisHandle,
    /// Shared HTTP client used by upstream proxies (Polymarket Gamma,
    /// Coinbase / Kraken / Binance spot tickers). Connection pool is shared
    /// across requests so we don't re-handshake on every call.
    pub gamma: reqwest::Client,
    /// Root directory whose immediate subfolders the `/api/monitor` endpoint
    /// reports du-style usage for (`backend.data_dir`, default `/data`).
    pub data_dir: std::path::PathBuf,
}
