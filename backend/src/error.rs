//! Unified error type for handlers + its `IntoResponse` impl.
//!
//! Each variant maps to a specific HTTP status:
//!   - `NotFound` → 404
//!   - `Decode`   → 500 (we got bytes back but couldn't parse them)
//!   - `Redis`    → 500 (Redis itself errored)
//!   - `Network`  → 502 (upstream HTTP call failed)

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::Serialize;

pub enum ApiError {
    NotFound(String),
    Decode(String),
    Redis(String),
    Network(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Decode(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            ApiError::Redis(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            ApiError::Network(m) => (StatusCode::BAD_GATEWAY, m),
        };
        (status, Json(ErrorBody { error: msg })).into_response()
    }
}
