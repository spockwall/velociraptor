//! Per-exchange REST order client trait + factory.
//!
//! The only concrete implementation today is `polymarket.rs`. The
//! `RestOrderClient` trait is the single surface the ZMQ gateway dispatches
//! against — every `OrderAction` variant maps to exactly one trait method.

use std::cell::Cell;

use async_trait::async_trait;
use libs::protocol::orders::{HeartbeatAck, OrderAck, OrderError, OrderStatus, PlaceOne};

pub mod kalshi;
pub mod polymarket;
pub mod retry;

tokio::task_local! {
    /// Per-request scratch cell for the venue-facing CLOB HTTP round-trip time
    /// (ms). The REST client (deep inside `place()`) writes it via
    /// [`record_venue_ms`] around the exchange HTTP call — on BOTH the accept
    /// and the 4xx-reject branch — and `Executor::handle_request` reads it back
    /// with [`take_venue_ms`] after dispatch to stamp `OrderResponse.venue_ms`.
    ///
    /// A task-local is used (rather than widening the `RestOrderClient::place`
    /// signature) so the error path — which discards the `OrderAck` — can still
    /// carry the timing out, and so no order/TIF/signing logic or trait shape
    /// changes. Each request runs in its own `tokio::spawn` task
    /// (`gateway::Gateway::run`), so the cell is naturally per-request isolated.
    pub static VENUE_MS: Cell<Option<f64>>;
}

/// Record the venue HTTP round-trip time (ms) for the current request, if the
/// task is running inside a [`VENUE_MS`] scope. A no-op otherwise (e.g. unit
/// tests calling `place()` directly), so it can never panic on the order path.
pub fn record_venue_ms(ms: f64) {
    let _ = VENUE_MS.try_with(|c| c.set(Some(ms)));
}

/// Consume the venue timing recorded for the current request. Returns `None`
/// outside a [`VENUE_MS`] scope or when nothing was recorded (no HTTP call
/// reached).
pub fn take_venue_ms() -> Option<f64> {
    VENUE_MS.try_with(|c| c.take()).unwrap_or(None)
}

/// Resolved target/strike price for one market. Whichever fields the upstream
/// returns are populated; absent fields are `None`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TargetPrice {
    /// Single strike (Polymarket `line`, Kalshi `strike_value`).
    pub line: Option<f64>,
    /// Lower bound for range markets (Polymarket `lowerBound`, Kalshi `floor_strike`).
    pub lower: Option<f64>,
    /// Upper bound for range markets (Polymarket `upperBound`, Kalshi `cap_strike`).
    pub upper: Option<f64>,
}

/// REST order client surface. One implementor per exchange.
#[async_trait]
pub trait RestOrderClient: Send + Sync {
    async fn place(&self, o: &PlaceOne) -> Result<OrderAck, OrderError>;
    async fn place_batch(
        &self,
        os: &[PlaceOne],
    ) -> Result<Vec<Result<OrderAck, OrderError>>, OrderError>;
    async fn update(
        &self,
        client_oid: &str,
        exchange_oid: &str,
        new_px: Option<f64>,
        new_qty: Option<f64>,
    ) -> Result<OrderAck, OrderError>;
    async fn cancel(&self, exchange_oid: &str) -> Result<OrderAck, OrderError>;
    async fn cancel_all(&self) -> Result<u32, OrderError>;
    async fn cancel_market(&self, symbol: &str) -> Result<u32, OrderError>;
    async fn get_order(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError>;
    async fn get_orders(&self) -> Result<Vec<OrderAck>, OrderError>;
    async fn order_status(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError>;
    async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError>;
}
