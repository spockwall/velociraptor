//! ZMQ ROUTER gateway.
//!
//! The trading engine connects a `DEALER` socket to this `ROUTER`. ROUTER
//! preserves the conceptual one-direction request channel while letting the
//! executor pipeline concurrent REST calls.
//!
//! Frames (ROUTER side):
//!   recv: [ identity | empty | msgpack(OrderRequest) ]
//!   send: [ identity | empty | msgpack(OrderResponse) ]
//!
//! Loop:
//!   1. Read a multipart frame from the ROUTER socket on a blocking thread.
//!   2. Hand off to a tokio task that decodes, applies kill-switch + idempotency
//!      gates, dispatches against the per-exchange `RestOrderClient`, and
//!      enqueues the response to a writer task.
//!   3. Writer task encodes the response and sends it back through ROUTER
//!      (still on a blocking thread; ROUTER sockets aren't tokio-aware).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use libs::protocol::orders::{
    OrderAction, OrderError, OrderRequest, OrderResponse, OrderResult, OrderStatus,
};
use libs::protocol::ExchangeName;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub mod idempotency;

use crate::control::{ControlState, ShutdownState};
use crate::gateway::idempotency::{IdempotencyCache, Kind as IdemKind};
use crate::ops::AuditSink;
use crate::rest::RestOrderClient;

pub type ClientMap = HashMap<ExchangeName, Arc<dyn RestOrderClient>>;

pub struct GatewayConfig {
    pub bind: String,
}

pub struct Gateway {
    config: GatewayConfig,
    clients: Arc<ClientMap>,
    audit: Arc<AuditSink>,
    control: Arc<ControlState>,
    idempotency: Arc<IdempotencyCache>,
    shutdown: Arc<ShutdownState>,
}

impl Gateway {
    pub fn new(
        config: GatewayConfig,
        clients: Arc<ClientMap>,
        audit: Arc<AuditSink>,
        control: Arc<ControlState>,
        idempotency: Arc<IdempotencyCache>,
        shutdown: Arc<ShutdownState>,
    ) -> Self {
        Self {
            config,
            clients,
            audit,
            control,
            idempotency,
            shutdown,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        // Channels:
        //   req:  router_thread → tokio dispatcher
        //   resp: tokio dispatcher → router_thread
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(Vec<u8>, Vec<u8>)>();
        let (resp_tx, resp_rx_async) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();

        // The single thread that owns the ROUTER socket. zmq sockets aren't
        // thread-safe, so reads + writes happen here. Polls req_rx with a
        // short timeout, drains pending responses between recvs.
        let bind = self.config.bind.clone();
        let shutdown_for_router = self.shutdown.clone();
        let resp_rx_blocking = std::sync::Arc::new(std::sync::Mutex::new(resp_rx_async));
        let resp_rx_for_thread = resp_rx_blocking.clone();
        let router_thread = std::thread::spawn(move || {
            let ctx = zmq::Context::new();
            let sock = ctx.socket(zmq::ROUTER).expect("ROUTER socket");
            sock.bind(&bind).expect("bind ROUTER");
            sock.set_rcvtimeo(50).ok();
            info!("zmq_gateway: ROUTER bound on {bind}");
            loop {
                if shutdown_for_router.is_draining()
                    && resp_rx_for_thread.lock().unwrap().is_empty()
                {
                    break;
                }
                // Drain pending responses first (cheap, non-blocking).
                {
                    let mut rx = resp_rx_for_thread.lock().unwrap();
                    while let Ok((identity, bytes)) = rx.try_recv() {
                        if let Err(e) = sock.send(&identity[..], zmq::SNDMORE) {
                            warn!("zmq_gateway: send identity failed: {e}");
                            continue;
                        }
                        if let Err(e) = sock.send(&[][..], zmq::SNDMORE) {
                            warn!("zmq_gateway: send delim failed: {e}");
                            continue;
                        }
                        if let Err(e) = sock.send(&bytes[..], 0) {
                            warn!("zmq_gateway: send body failed: {e}");
                        }
                    }
                }
                // Then attempt to recv one new request.
                match sock.recv_multipart(0) {
                    Ok(parts) => {
                        if parts.len() < 2 {
                            warn!("zmq_gateway: dropping short frame ({} parts)", parts.len());
                            continue;
                        }
                        let identity = parts[0].clone();
                        let payload = parts.last().cloned().unwrap_or_default();
                        if req_tx.send((identity, payload)).is_err() {
                            break;
                        }
                    }
                    Err(zmq::Error::EAGAIN) => continue,
                    Err(e) => warn!("zmq_gateway: recv error {e:?}"),
                }
            }
        });

        // ── Tokio dispatcher: bridge std::mpsc → tokio::mpsc, then pump ──
        let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();
        let shutdown_for_bridge = self.shutdown.clone();
        std::thread::spawn(move || {
            while let Ok(msg) = req_rx.recv() {
                if bridge_tx.send(msg).is_err() {
                    break;
                }
                if shutdown_for_bridge.is_draining() {
                    break;
                }
            }
        });

        let clients = self.clients.clone();
        let audit = self.audit.clone();
        let control = self.control.clone();
        let idem = self.idempotency.clone();
        let shutdown_state = self.shutdown.clone();
        while let Some((identity, payload)) = bridge_rx.recv().await {
            self.shutdown.inc_inflight();
            let clients = clients.clone();
            let audit = audit.clone();
            let control = control.clone();
            let idem = idem.clone();
            let shutdown_state2 = shutdown_state.clone();
            let resp_tx_clone = resp_tx.clone();
            tokio::spawn(async move {
                let resp = handle_one(&payload, &clients, &audit, &control, &idem).await;
                match rmp_serde::to_vec_named(&resp) {
                    Ok(bytes) => {
                        let _ = resp_tx_clone.send((identity, bytes));
                    }
                    Err(e) => warn!("zmq_gateway: encode response failed: {e}"),
                }
                shutdown_state2.dec_inflight();
            });
        }

        let _ = router_thread.join();
        Ok(())
    }
}

async fn handle_one(
    payload: &[u8],
    clients: &ClientMap,
    audit: &AuditSink,
    control: &ControlState,
    idem: &IdempotencyCache,
) -> OrderResponse {
    // 1. Decode.
    let req: OrderRequest = match rmp_serde::from_slice(payload) {
        Ok(r) => r,
        Err(e) => {
            warn!("zmq_gateway: decode failed: {e}");
            return OrderResponse {
                req_id: 0,
                result: Err(OrderError::Internal {
                    message: format!("decode: {e}"),
                }),
            };
        }
    };

    audit.log_request(&req).await;

    // 2. Idempotency replay (Place / PlaceBatch only — keyed by client_oid).
    let idem_key = match &req.action {
        OrderAction::Place(p) => Some((p.client_oid.clone(), IdemKind::Place)),
        _ => None,
    };
    if let Some((ref key, _kind)) = idem_key {
        if let Some(cached) = idem.get(key) {
            debug!("zmq_gateway: idempotency hit for client_oid={key}");
            return OrderResponse {
                req_id: req.req_id,
                result: cached.result,
            };
        }
    }

    // 3. Kill-switch gate: only Cancel-class actions are allowed.
    if control.is_blocked(req.exchange) && !is_cancel(&req.action) {
        let resp = OrderResponse {
            req_id: req.req_id,
            result: Err(OrderError::KillSwitch),
        };
        audit.log_response(req.req_id, &resp.result, true).await;
        return resp;
    }

    // 4. Dispatch.
    let client = match clients.get(&req.exchange) {
        Some(c) => c.clone(),
        None => {
            let resp = OrderResponse {
                req_id: req.req_id,
                result: Err(OrderError::Internal {
                    message: format!("no client configured for {}", req.exchange),
                }),
            };
            audit.log_response(req.req_id, &resp.result, true).await;
            return resp;
        }
    };

    let result = dispatch(&*client, &req.action).await;
    let resp = OrderResponse {
        req_id: req.req_id,
        result: result.clone(),
    };

    // 5. Audit + idempotency stash.
    let critical = matches!(
        &result,
        Err(OrderError::ExchangeRejected { .. } | OrderError::Internal { .. })
    );
    audit.log_response(req.req_id, &result, critical).await;
    if let Some((key, kind)) = idem_key {
        idem.insert(key, resp.clone(), kind);
    }
    resp
}


fn is_cancel(a: &OrderAction) -> bool {
    matches!(
        a,
        OrderAction::Cancel { .. } | OrderAction::CancelAll | OrderAction::CancelMarket { .. }
    )
}

async fn dispatch(
    client: &dyn RestOrderClient,
    action: &OrderAction,
) -> Result<OrderResult, OrderError> {
    match action {
        OrderAction::Place(p) => client.place(p).await.map(OrderResult::Ack),
        OrderAction::PlaceBatch { orders } => {
            let results = client.place_batch(orders).await?;
            Ok(OrderResult::BatchAck { results })
        }
        OrderAction::Update {
            client_oid,
            exchange_oid,
            new_px,
            new_qty,
        } => client
            .update(client_oid, exchange_oid, *new_px, *new_qty)
            .await
            .map(OrderResult::Ack),
        OrderAction::Cancel { exchange_oid } => client
            .cancel(exchange_oid)
            .await
            .map(|_ack| OrderResult::CancelCount { count: 1 }),
        OrderAction::CancelAll => client
            .cancel_all()
            .await
            .map(|count| OrderResult::CancelCount { count }),
        OrderAction::CancelMarket { symbol } => client
            .cancel_market(symbol)
            .await
            .map(|count| OrderResult::CancelCount { count }),
        OrderAction::Heartbeat => client.heartbeat().await.map(OrderResult::HeartbeatOk),
    }
}

// Keep `Ordering` and `OrderStatus` imports referenced (used in path expansion).
#[allow(dead_code)]
fn _anchor() -> Option<OrderStatus> {
    let _ = Ordering::Relaxed;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use libs::protocol::orders::{
        HeartbeatAck, OrderAck, OrderError, OrderKind, OrderStatus, PlaceOne, Side, Tif,
    };
    use libs::protocol::ExchangeName;
    use std::sync::atomic::AtomicU32;
    use tempfile::tempdir;

    struct StubClient {
        place_count: AtomicU32,
    }

    #[async_trait]
    impl RestOrderClient for StubClient {
        async fn place(&self, o: &PlaceOne) -> Result<OrderAck, OrderError> {
            self.place_count.fetch_add(1, Ordering::Relaxed);
            Ok(OrderAck {
                client_oid: o.client_oid.clone(),
                exchange_oid: format!("ex-{}", o.client_oid),
                status: OrderStatus::New,
                ts_ns: 0,
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
        async fn cancel(&self, exchange_oid: &str) -> Result<OrderAck, OrderError> {
            Ok(OrderAck {
                client_oid: "".into(),
                exchange_oid: exchange_oid.into(),
                status: OrderStatus::Canceled,
                ts_ns: 0,
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
            Ok(vec![])
        }
        async fn order_status(&self, _exchange_oid: &str) -> Result<OrderStatus, OrderError> {
            Ok(OrderStatus::New)
        }
        async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError> {
            Ok(HeartbeatAck { next_due_ms: 1000 })
        }
    }

    fn place_req() -> OrderRequest {
        OrderRequest {
            req_id: 7,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Place(PlaceOne {
                client_oid: "c1".into(),
                symbol: "X.YES".into(),
                side: Side::Buy,
                kind: OrderKind::Limit,
                px: 0.5,
                qty: 1.0,
                tif: Tif::Gtc,
            }),
        }
    }

    #[tokio::test]
    async fn place_dispatched_and_cached() {
        let dir = tempdir().unwrap();
        let audit = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        let stub = Arc::new(StubClient {
            place_count: AtomicU32::new(0),
        });
        let mut clients: ClientMap = HashMap::new();
        clients.insert(
            ExchangeName::Kalshi,
            stub.clone() as Arc<dyn RestOrderClient>,
        );
        let control = ControlState::default();
        let idem = IdempotencyCache::with_capacity(8);

        let payload = rmp_serde::to_vec_named(&place_req()).unwrap();
        let resp = handle_one(&payload, &clients, &audit, &control, &idem).await;
        assert_eq!(resp.req_id, 7);
        assert!(resp.result.is_ok());

        // Replay should hit the cache, not the stub.
        let resp2 = handle_one(&payload, &clients, &audit, &control, &idem).await;
        assert_eq!(resp2.req_id, 7);
        assert_eq!(stub.place_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn kill_switch_blocks_place() {
        let dir = tempdir().unwrap();
        let audit = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        let stub = Arc::new(StubClient {
            place_count: AtomicU32::new(0),
        });
        let mut clients: ClientMap = HashMap::new();
        clients.insert(
            ExchangeName::Kalshi,
            stub.clone() as Arc<dyn RestOrderClient>,
        );
        let control = ControlState::default();
        control.kill_switch.store(true, Ordering::Relaxed);
        let idem = IdempotencyCache::with_capacity(8);

        let payload = rmp_serde::to_vec_named(&place_req()).unwrap();
        let resp = handle_one(&payload, &clients, &audit, &control, &idem).await;
        match resp.result {
            Err(OrderError::KillSwitch) => {}
            other => panic!("expected KillSwitch, got {:?}", other),
        }
        assert_eq!(stub.place_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn cancel_passes_kill_switch() {
        let dir = tempdir().unwrap();
        let audit = AuditSink::open(dir.path().to_path_buf(), None, 1000)
            .await
            .unwrap();
        let stub = Arc::new(StubClient {
            place_count: AtomicU32::new(0),
        });
        let mut clients: ClientMap = HashMap::new();
        clients.insert(
            ExchangeName::Kalshi,
            stub.clone() as Arc<dyn RestOrderClient>,
        );
        let control = ControlState::default();
        control.kill_switch.store(true, Ordering::Relaxed);
        let idem = IdempotencyCache::with_capacity(8);

        let req = OrderRequest {
            req_id: 9,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Cancel {
                exchange_oid: "x1".into(),
            },
        };
        let payload = rmp_serde::to_vec_named(&req).unwrap();
        let resp = handle_one(&payload, &clients, &audit, &control, &idem).await;
        assert!(resp.result.is_ok());
    }
}
