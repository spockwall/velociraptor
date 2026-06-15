//! Boot-time reconciliation of live exchange orders against the
//! executor's [`OrderRegistry`].
//!
//! For each exchange:
//!   1. The registry must have been rehydrated from the audit log
//!      already (`Executor::rehydrate_registry`).
//!   2. Call `client.get_orders()` and compare against
//!      `registry.live_set(exchange)`.
//!   3. Surface mismatches as `Synthetic { op: "reconcile_*" }` audit
//!      events. The executor never auto-cancels — the operator decides.

use std::collections::HashSet;
use std::sync::Arc;

use libs::protocol::orders::OrderAck;
use libs::protocol::ExchangeName;
use serde_json::json;
use tracing::{info, warn};

use crate::ops::AuditSink;
use crate::registry::OrderRegistry;
use crate::rest::RestOrderClient;

/// Cross-check live `get_orders()` against the registry's `live_set`.
/// Logs synthetic events; returns `(unknown_on_exchange, lost_locally)`.
pub async fn reconcile_one(
    audit: &AuditSink,
    exchange: ExchangeName,
    client: Arc<dyn RestOrderClient>,
    registry: &OrderRegistry,
) -> (usize, usize) {
    let known_live: HashSet<String> = registry.live_set(exchange);

    let live: Vec<OrderAck> = match client.get_orders().await {
        Ok(v) => v,
        Err(e) => {
            warn!("reconcile {exchange}: get_orders failed: {e:?}");
            audit
                .log_synthetic(
                    "reconcile_skip",
                    json!({ "exchange": exchange.to_str(), "error": format!("{e:?}") }),
                )
                .await;
            return (0, 0);
        }
    };

    let live_set: HashSet<String> = live.iter().map(|a| a.exchange_oid.clone()).collect();

    let mut unknown_on_exchange = 0;
    for oid in &live_set {
        if !known_live.contains(oid) {
            unknown_on_exchange += 1;
            audit
                .log_synthetic(
                    "reconcile_unknown_order",
                    json!({ "exchange": exchange.to_str(), "exchange_oid": oid }),
                )
                .await;
        }
    }

    let mut lost = 0;
    for oid in &known_live {
        if !live_set.contains(oid) {
            lost += 1;
            audit
                .log_synthetic(
                    "reconcile_lost_order",
                    json!({ "exchange": exchange.to_str(), "exchange_oid": oid }),
                )
                .await;
        }
    }

    info!(
        "reconcile {exchange}: live={} known={} unknown_on_exchange={} lost={}",
        live_set.len(),
        known_live.len(),
        unknown_on_exchange,
        lost
    );
    (unknown_on_exchange, lost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use libs::protocol::orders::{HeartbeatAck, OrderError, OrderStatus, PlaceOne};
    use tempfile::tempdir;

    struct MockClient {
        orders: Vec<OrderAck>,
    }

    #[async_trait]
    impl RestOrderClient for MockClient {
        async fn place(&self, _o: &PlaceOne) -> Result<OrderAck, OrderError> {
            unimplemented!()
        }
        async fn place_batch(
            &self,
            _os: &[PlaceOne],
        ) -> Result<Vec<Result<OrderAck, OrderError>>, OrderError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _client_oid: &str,
            _exchange_oid: &str,
            _new_px: Option<f64>,
            _new_qty: Option<f64>,
        ) -> Result<OrderAck, OrderError> {
            unimplemented!()
        }
        async fn cancel(&self, _exchange_oid: &str) -> Result<OrderAck, OrderError> {
            Ok(OrderAck {
                client_oid: "".into(),
                exchange_oid: "".into(),
                status: OrderStatus::Canceled,
                ts_ns: 0,
                fill: None,
            })
        }
        async fn cancel_all(&self) -> Result<u32, OrderError> {
            Ok(0)
        }
        async fn cancel_market(&self, _symbol: &str) -> Result<u32, OrderError> {
            Ok(0)
        }
        async fn get_order(&self, _exchange_oid: &str) -> Result<OrderStatus, OrderError> {
            Ok(OrderStatus::New)
        }
        async fn get_orders(&self) -> Result<Vec<OrderAck>, OrderError> {
            Ok(self.orders.clone())
        }
        async fn order_status(&self, _exchange_oid: &str) -> Result<OrderStatus, OrderError> {
            Ok(OrderStatus::New)
        }
        async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError> {
            Ok(HeartbeatAck { next_due_ms: 0 })
        }
    }

    #[tokio::test]
    async fn reports_unknown_and_lost() {
        use crate::registry::{IdemKind, OrderEntry};
        use libs::protocol::orders::OrderResponse;
        use std::time::{Duration, Instant};

        let dir = tempdir().unwrap();
        let audit = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        let client = Arc::new(MockClient {
            orders: vec![OrderAck {
                client_oid: "".into(),
                exchange_oid: "live-1".into(),
                status: OrderStatus::New,
                ts_ns: 0,
                fill: None,
            }],
        });

        // Registry knows about a "known-but-gone" exchange_oid.
        let registry = OrderRegistry::with_capacity(8);
        registry.seed(
            "known-but-gone".to_string(),
            OrderEntry {
                exchange: ExchangeName::Kalshi,
                symbol: Some("X".into()),
                response: OrderResponse {
                    req_id: 0,
                    result: Err(OrderError::Timeout),
                    meta: None,
                },
                exchange_oid: Some("known-but-gone".into()),
                status: OrderStatus::New,
                placed_at: Instant::now(),
                kind: IdemKind::Place,
                idem_expires_at: Instant::now() + Duration::from_secs(60),
            },
        );

        let (unknown, lost) =
            reconcile_one(&audit, ExchangeName::Kalshi, client, &registry).await;
        assert_eq!(unknown, 1);
        assert_eq!(lost, 1);
    }
}
