//! Per-exchange REST order client trait + factory.
//!
//! Concrete implementations live in `kalshi.rs` and `polymarket.rs`. The
//! `RestOrderClient` trait is the single surface the ZMQ gateway dispatches
//! against — every `OrderAction` variant maps to exactly one trait method.

use async_trait::async_trait;
use libs::protocol::orders::{HeartbeatAck, OrderAck, OrderError, OrderStatus, PlaceOne};

pub mod kalshi;
pub mod polymarket;
pub mod retry;

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
