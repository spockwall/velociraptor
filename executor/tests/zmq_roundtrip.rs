//! In-process ZMQ DEALER → ROUTER round-trip smoke test.
//!
//! Spawns the executor's `Gateway` (built on top of an `Executor`
//! orchestrator) against an in-memory stub client, sends a msgpack
//! `OrderRequest`, and asserts the typed `OrderResponse` round-trips.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use executor::control::{ControlState, ShutdownState};
use executor::gateway::{ClientMap, Gateway, GatewayConfig};
use executor::ops::{AuditSink, Metrics};
use executor::rest::RestOrderClient;
use executor::{Executor, ExecutorBuild};
use libs::protocol::orders::{
    HeartbeatAck, OrderAck, OrderAction, OrderError, OrderKind, OrderRequest, OrderResponse,
    OrderResult, OrderStatus, PlaceOne, Side, Tif,
};
use libs::protocol::ExchangeName;
use tempfile::tempdir;

struct StubClient {
    place_count: AtomicU32,
}

#[async_trait]
impl RestOrderClient for StubClient {
    async fn place(&self, p: &PlaceOne) -> Result<OrderAck, OrderError> {
        self.place_count.fetch_add(1, Ordering::Relaxed);
        Ok(OrderAck {
            client_oid: p.client_oid.clone(),
            exchange_oid: format!("ex-{}", p.client_oid),
            status: OrderStatus::New,
            ts_ns: 0,
            fill: None,
        })
    }
    async fn place_batch(
        &self,
        _os: &[PlaceOne],
    ) -> Result<Vec<Result<OrderAck, OrderError>>, OrderError> {
        Ok(vec![])
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
        unimplemented!()
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
        Ok(vec![])
    }
    async fn order_status(&self, _exchange_oid: &str) -> Result<OrderStatus, OrderError> {
        Ok(OrderStatus::New)
    }
    async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError> {
        Ok(HeartbeatAck { next_due_ms: 5000 })
    }
}

async fn make_gateway(bind: &str) -> (Gateway, Arc<StubClient>, Arc<ShutdownState>) {
    let dir = tempdir().unwrap();
    let audit = Arc::new(
        AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap(),
    );
    let stub = Arc::new(StubClient {
        place_count: AtomicU32::new(0),
    });
    let mut clients: ClientMap = HashMap::new();
    clients.insert(
        ExchangeName::Kalshi,
        stub.clone() as Arc<dyn RestOrderClient>,
    );
    let control = Arc::new(ControlState::default());
    let shutdown = Arc::new(ShutdownState::default());
    let executor = Arc::new(Executor::new(ExecutorBuild {
        clients,
        audit,
        metrics: Arc::new(Metrics::new()),
        control,
        shutdown: shutdown.clone(),
        redis: None,
        config_path: PathBuf::from("/tmp/nonexistent.yaml"),
    }));
    let gw = Gateway::new(
        GatewayConfig {
            bind: bind.to_string(),
        },
        executor,
    );
    (gw, stub, shutdown)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dealer_router_heartbeat() {
    let port = 5557 + (std::process::id() % 1000);
    let bind = format!("tcp://127.0.0.1:{port}");
    let connect = format!("tcp://127.0.0.1:{port}");

    let (gw, _stub, shutdown) = make_gateway(&bind).await;
    let gw_handle = tokio::spawn(async move { gw.run().await });

    tokio::time::sleep(Duration::from_millis(150)).await;

    let connect_clone = connect.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<OrderResponse> {
        let ctx = zmq::Context::new();
        let dealer = ctx.socket(zmq::DEALER)?;
        dealer.set_rcvtimeo(2000)?;
        dealer.set_sndtimeo(2000)?;
        dealer.connect(&connect_clone)?;

        let req = OrderRequest {
            req_id: 42,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Heartbeat,
        };
        let payload = rmp_serde::to_vec_named(&req)?;
        dealer.send(&[][..], zmq::SNDMORE)?;
        dealer.send(&payload[..], 0)?;

        let parts = dealer.recv_multipart(0)?;
        let resp_bytes = parts.last().cloned().unwrap_or_default();
        let resp: OrderResponse = rmp_serde::from_slice(&resp_bytes)?;
        Ok(resp)
    })
    .await
    .unwrap();

    let resp = result.expect("dealer round-trip");
    assert_eq!(resp.req_id, 42);
    match resp.result {
        Ok(OrderResult::HeartbeatOk(hb)) => assert!(hb.next_due_ms > 0),
        other => panic!("expected HeartbeatOk, got {:?}", other),
    }

    shutdown
        .draining
        .store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    gw_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn place_round_trip() {
    let port = 6557 + (std::process::id() % 1000);
    let bind = format!("tcp://127.0.0.1:{port}");
    let connect = bind.clone();

    let (gw, stub, shutdown) = make_gateway(&bind).await;
    let gw_handle = tokio::spawn(async move { gw.run().await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let req = OrderRequest {
        req_id: 1,
        exchange: ExchangeName::Kalshi,
        action: OrderAction::Place(PlaceOne {
            client_oid: "c-1".into(),
            symbol: "X.YES".into(),
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.5,
            qty: 1.0,
            tif: Tif::Gtc,
        }),
    };
    let payload = rmp_serde::to_vec_named(&req).unwrap();

    let resp: OrderResponse = tokio::task::spawn_blocking(move || {
        let ctx = zmq::Context::new();
        let dealer = ctx.socket(zmq::DEALER).unwrap();
        dealer.set_rcvtimeo(2000).unwrap();
        dealer.set_sndtimeo(2000).unwrap();
        dealer.connect(&connect).unwrap();
        dealer.send(&[][..], zmq::SNDMORE).unwrap();
        dealer.send(&payload[..], 0).unwrap();
        let parts = dealer.recv_multipart(0).unwrap();
        rmp_serde::from_slice(parts.last().unwrap()).unwrap()
    })
    .await
    .unwrap();

    assert_eq!(resp.req_id, 1);
    match resp.result {
        Ok(OrderResult::Ack(ack)) => {
            assert_eq!(ack.client_oid, "c-1");
            assert_eq!(ack.exchange_oid, "ex-c-1");
        }
        other => panic!("expected Ack, got {:?}", other),
    }
    assert_eq!(stub.place_count.load(Ordering::Relaxed), 1);

    shutdown
        .draining
        .store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    gw_handle.abort();
}
