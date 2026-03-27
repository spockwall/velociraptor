// orderbook/src/api/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("[Orderbook] Exchange not found: {0}")]
    ExchangeNotFound(String),

    #[error("[Orderbook] Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("[Orderbook] Connection error: {0}")]
    ConnectionError(String),

    #[error("[Orderbook] Already running")]
    AlreadyRunning,

    #[error("[Orderbook] Not running")]
    NotRunning,

    #[error("[Orderbook] Channel error: {0}")]
    ChannelError(String),

    #[error("[Orderbook] Storage error: {0}")]
    StorageError(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;
