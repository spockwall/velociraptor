//! Error mapping: `reqwest::Error` and HTTP statuses → `libs::protocol::orders::OrderError`.

use libs::protocol::orders::OrderError;

/// Map a `reqwest::Error` (transport failure before we got a status code) into the
/// typed wire error.
pub fn map_reqwest(err: reqwest::Error) -> OrderError {
    if err.is_timeout() {
        OrderError::Timeout
    } else if err.is_connect() {
        OrderError::Network {
            message: format!("connect: {err}"),
        }
    } else {
        OrderError::Network {
            message: err.to_string(),
        }
    }
}

/// Map an HTTP error response (status >= 400) plus body into `OrderError`.
pub fn map_http_status(status: reqwest::StatusCode, body: &str) -> OrderError {
    if status.as_u16() == 404 {
        OrderError::NotFound {
            exchange_oid: String::new(),
        }
    } else {
        OrderError::ExchangeRejected {
            code: Some(status.as_u16().to_string()),
            message: body.to_string(),
        }
    }
}

/// Map a serde / parse failure to `OrderError::Internal`.
pub fn map_internal<E: std::fmt::Display>(err: E) -> OrderError {
    OrderError::Internal {
        message: err.to_string(),
    }
}
