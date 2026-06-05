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
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

/// `TraceLayer` whose request/response (success) events log at `level`.
/// Server-error (5xx) responses always log at ERROR regardless, so they
/// still reach `error.log` even when `level` is DEBUG.
fn trace_layer(
    level: Level,
) -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
> {
    TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(level))
        .on_request(DefaultOnRequest::new().level(level))
        .on_response(DefaultOnResponse::new().level(level))
    // on_failure is left at its default (ERROR) — 5xx still hits error.log.
}

pub fn router(state: Arc<AppState>) -> Router {
    // High-frequency orderbook read endpoints — traced at DEBUG so they don't
    // flood the INFO log. Errors still surface (on_failure = ERROR).
    let orderbook_reads = Router::new()
        .route("/health", get(routes::orderbook::health))
        .route("/api/orderbook/:exchange/:symbol", get(routes::orderbook::get_orderbook))
        .route("/api/bba/:exchange/:symbol", get(routes::orderbook::get_bba))
        .route("/api/snapshots/:exchange/:symbol", get(routes::orderbook::get_snapshots))
        .route("/api/trades/:exchange/:symbol", get(routes::orderbook::get_trades))
        .layer(trace_layer(Level::DEBUG));

    // Everything else — control, markets, events, spot — traced at INFO.
    let other = Router::new()
        .route("/api/polymarket/markets", get(routes::markets::get_polymarket_markets))
        .route("/api/kalshi/markets", get(routes::markets::get_kalshi_markets))
        .route("/api/events/fills", get(routes::events::get_fills))
        .route("/api/events/orders", get(routes::events::get_orders))
        .route(
            "/api/control",
            get(routes::control::get_control).post(routes::control::post_control),
        )
        .route("/api/monitor", get(routes::monitor::get_monitor))
        .route("/api/monitor/history", get(routes::monitor::get_monitor_history))
        .route("/api/logs/errors", get(routes::logs::get_error_logs))
        .route("/api/spot_price/:product", get(routes::spot::get_spot_price))
        .route(
            "/api/window_open_price/:product/:interval_secs/:window_start",
            get(routes::spot::get_window_open_price),
        )
        .layer(trace_layer(Level::INFO));

    orderbook_reads.merge(other).with_state(state)
}
