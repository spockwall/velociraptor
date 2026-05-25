//! ZMQ ROUTER gateway — transport adapter only.
//!
//! The trading engine connects a `DEALER` socket to this `ROUTER`. ROUTER
//! preserves the conceptual one-direction request channel while letting
//! the executor pipeline concurrent REST calls.
//!
//! Frames (ROUTER side):
//!   recv: [ identity | empty | msgpack(OrderRequest) ]
//!   send: [ identity | empty | msgpack(OrderResponse) ]
//!
//! Loop:
//!   1. Read a multipart frame from the ROUTER socket on a blocking thread.
//!   2. Hand the payload to [`crate::Executor::handle_one`] for the entire
//!      per-request pipeline.
//!   3. Encoded response goes back through ROUTER via a writer that
//!      shares the same blocking thread (zmq sockets aren't thread-safe).
//!
//! All non-transport concerns (audit, idempotency, kill-switch, risk,
//! dispatch, registry, metrics) live on `Executor`. This module is just
//! plumbing.

use std::collections::HashMap;
use std::sync::Arc;

use libs::protocol::ExchangeName;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::rest::RestOrderClient;
use crate::Executor;

pub type ClientMap = HashMap<ExchangeName, Arc<dyn RestOrderClient>>;

pub struct GatewayConfig {
    pub bind: String,
}

pub struct Gateway {
    config: GatewayConfig,
    executor: Arc<Executor>,
}

impl Gateway {
    pub fn new(config: GatewayConfig, executor: Arc<Executor>) -> Self {
        Self { config, executor }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(Vec<u8>, Vec<u8>)>();
        let (resp_tx, resp_rx_async) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();

        // ROUTER socket thread (zmq is not thread-safe).
        let bind = self.config.bind.clone();
        let shutdown_for_router = self.executor.shutdown().clone();
        let resp_rx_blocking = Arc::new(std::sync::Mutex::new(resp_rx_async));
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
                // Drain pending responses first.
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
                // Attempt to recv one new request.
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

        // Bridge std::mpsc → tokio::mpsc.
        let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();
        let shutdown_for_bridge = self.executor.shutdown().clone();
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

        // Tokio dispatcher: one task per request → executor.handle_one.
        while let Some((identity, payload)) = bridge_rx.recv().await {
            self.executor.shutdown().inc_inflight();
            let executor = self.executor.clone();
            let shutdown_state2 = self.executor.shutdown().clone();
            let resp_tx_clone = resp_tx.clone();
            tokio::spawn(async move {
                let resp = executor.handle_request(&payload).await;
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
