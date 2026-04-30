//! Backend HTTP API — Axum service that reads Redis to expose market data.
//!
//! Module layout:
//!   - `state`   — `AppState` shared across handlers
//!   - `error`   — `ApiError` + `IntoResponse` glue
//!   - `routes`  — handlers grouped by feature (orderbook, markets, spot)

pub mod error;
pub mod routes;
pub mod state;

pub use error::ApiError;
pub use state::AppState;

use axum::{routing::get, Router};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(routes::orderbook::health))
        .route("/api/orderbook/:exchange/:symbol", get(routes::orderbook::get_orderbook))
        .route("/api/bba/:exchange/:symbol", get(routes::orderbook::get_bba))
        .route("/api/snapshots/:exchange/:symbol", get(routes::orderbook::get_snapshots))
        .route("/api/trades/:exchange/:symbol", get(routes::orderbook::get_trades))
        .route("/api/polymarket/markets", get(routes::markets::get_polymarket_markets))
        .route("/api/kalshi/markets", get(routes::markets::get_kalshi_markets))
        .route("/api/spot_price/:product", get(routes::spot::get_spot_price))
        .route(
            "/api/window_open_price/:product/:interval_secs/:window_start",
            get(routes::spot::get_window_open_price),
        )
        .with_state(state)
}
