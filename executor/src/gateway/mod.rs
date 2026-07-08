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
//!   1. `zmq_poll` over the ROUTER socket AND an inproc wakeup PULL on a
//!      blocking thread — the thread sleeps until either a new request or a
//!      finished response is ready (no rcvtimeo gap between an HTTP ack
//!      completing and the engine seeing it).
//!   2. Hand request payloads to [`crate::Executor::handle_request`] for the
//!      per-request pipeline (one tokio task each).
//!   3. Encoded responses flow tokio → forwarder thread → inproc PUSH →
//!      the poller thread, which writes them out the ROUTER immediately
//!      (zmq sockets aren't thread-safe, so both ROUTER and PULL live on
//!      the poller thread).
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
        let (resp_tx, resp_rx) = mpsc::unbounded_channel::<(Vec<u8>, Vec<u8>)>();

        // One shared zmq context — inproc transport is per-context, and the
        // response forwarder uses it to wake the poller thread.
        let ctx = zmq::Context::new();
        const RESP_INPROC: &str = "inproc://gateway-resp";

        // Poller thread: ROUTER + wakeup PULL (zmq sockets aren't
        // thread-safe, so both live and die here). The thread sleeps in
        // `zmq_poll` until a request OR a finished response is ready — a
        // response wakes it immediately instead of waiting out the old
        // 50 ms rcvtimeo window, which added a flat 0–50 ms to every ack
        // the (synchronously blocked) engine was waiting on.
        let bind = self.config.bind.clone();
        let shutdown_for_router = self.executor.shutdown().clone();
        let ctx_router = ctx.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let router_thread = std::thread::spawn(move || {
            let sock = ctx_router.socket(zmq::ROUTER).expect("ROUTER socket");
            sock.bind(&bind).expect("bind ROUTER");
            let pull = ctx_router.socket(zmq::PULL).expect("PULL socket");
            pull.bind(RESP_INPROC).expect("bind inproc resp");
            let _ = ready_tx.send(()); // inproc bound — forwarder may connect
            info!("zmq_gateway: ROUTER bound on {bind}");
            'outer: loop {
                let (req_ready, resp_ready) = {
                    let mut items = [
                        sock.as_poll_item(zmq::POLLIN),
                        pull.as_poll_item(zmq::POLLIN),
                    ];
                    // 100 ms cap so shutdown drain is noticed when idle.
                    match zmq::poll(&mut items, 100) {
                        Ok(_) => (items[0].is_readable(), items[1].is_readable()),
                        Err(e) => {
                            warn!("zmq_gateway: poll error {e:?}");
                            continue;
                        }
                    }
                };
                // Responses first — a blocked engine is waiting on them.
                if resp_ready {
                    loop {
                        match pull.recv_multipart(zmq::DONTWAIT) {
                            Ok(parts) if parts.len() == 2 => {
                                let identity = &parts[0];
                                let bytes = &parts[1];
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
                            Ok(parts) => warn!(
                                "zmq_gateway: dropping malformed response ({} parts)",
                                parts.len()
                            ),
                            Err(zmq::Error::EAGAIN) => break,
                            Err(e) => {
                                warn!("zmq_gateway: response recv error {e:?}");
                                break;
                            }
                        }
                    }
                }
                if req_ready {
                    loop {
                        match sock.recv_multipart(zmq::DONTWAIT) {
                            Ok(parts) => {
                                if parts.len() < 2 {
                                    warn!(
                                        "zmq_gateway: dropping short frame ({} parts)",
                                        parts.len()
                                    );
                                    continue;
                                }
                                let identity = parts[0].clone();
                                let payload = parts.last().cloned().unwrap_or_default();
                                if req_tx.send((identity, payload)).is_err() {
                                    break 'outer;
                                }
                            }
                            Err(zmq::Error::EAGAIN) => break,
                            Err(e) => {
                                warn!("zmq_gateway: recv error {e:?}");
                                break;
                            }
                        }
                    }
                }
                if shutdown_for_router.is_draining() && !req_ready && !resp_ready {
                    break;
                }
            }
        });
        // Wait for the inproc bind so the forwarder's connect can't race it.
        let _ = ready_rx.recv();

        // Response forwarder: tokio channel → inproc PUSH. A dedicated
        // blocking thread lets any number of response tasks send without
        // sharing a zmq socket; each PUSH wakes the poller instantly.
        let ctx_fwd = ctx.clone();
        std::thread::spawn(move || {
            let push = ctx_fwd.socket(zmq::PUSH).expect("PUSH socket");
            push.connect(RESP_INPROC).expect("connect inproc resp");
            let mut resp_rx = resp_rx;
            while let Some((identity, bytes)) = resp_rx.blocking_recv() {
                if let Err(e) = push.send_multipart([identity, bytes], 0) {
                    warn!("zmq_gateway: response forward failed: {e}");
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
