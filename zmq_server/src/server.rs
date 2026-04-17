//! Unified ZMQ server.
//!
//! Binds three sockets and runs a single dispatch loop:
//! - **PUB** `market_pub_endpoint` — orderbook snapshots (topic `{ex}:{sym}`),
//!   throttled per subscription.
//! - **ROUTER** `router_endpoint` — `subscribe` / `unsubscribe` / `add_channel`
//!   requests from DEALER clients.
//! - **PUB** `user_pub_endpoint` — private user events (topic
//!   `user.{ex}.{kind}`), one publish per event.
//!
//! The server consumes a single `broadcast::Receiver<EngineEvent>` obtained
//! via the `EngineEventSource` trait, so it is decoupled from the `orderbook`
//! crate.

use crate::control::{Registry, dispatch, handle_control};
use crate::frame::user as user_frame;
use crate::socket::{PubSocket, RouterSocket};
use crate::trading::events::{EngineEvent, EngineEventSource};
use crate::types::ChannelRequest;
use std::sync::Arc;
use std::time::Instant;
use tmq::Context;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

pub struct ZmqServer {
    pub market_pub_endpoint: String,
    pub router_endpoint: String,
    pub user_pub_endpoint: String,
    pub channel_tx: mpsc::UnboundedSender<ChannelRequest>,
}

impl ZmqServer {
    pub fn new(
        market_pub_endpoint: impl Into<String>,
        router_endpoint: impl Into<String>,
        user_pub_endpoint: impl Into<String>,
        channel_tx: mpsc::UnboundedSender<ChannelRequest>,
    ) -> Self {
        Self {
            market_pub_endpoint: market_pub_endpoint.into(),
            router_endpoint: router_endpoint.into(),
            user_pub_endpoint: user_pub_endpoint.into(),
            channel_tx,
        }
    }

    /// Spawn the server task. Keep the returned `JoinHandle` alive.
    pub fn start(self, source: Arc<dyn EngineEventSource>) -> tokio::task::JoinHandle<()> {
        let mut event_rx = source.subscribe();

        tokio::spawn(async move {
            let ctx = Context::new();

            let mut market_pub = match PubSocket::bind(&ctx, &self.market_pub_endpoint, "market") {
                Ok(s) => s,
                Err(e) => {
                    error!("ZMQ market PUB bind failed: {e}");
                    return;
                }
            };

            let mut router_sock = match RouterSocket::bind(&ctx, &self.router_endpoint) {
                Ok(s) => s,
                Err(e) => {
                    error!("ZMQ ROUTER bind failed: {e}");
                    return;
                }
            };

            let mut user_pub = match PubSocket::bind(&ctx, &self.user_pub_endpoint, "user") {
                Ok(s) => s,
                Err(e) => {
                    error!("ZMQ user PUB bind failed: {e}");
                    return;
                }
            };

            let mut registry = Registry::new();
            let channel_tx = self.channel_tx;

            loop {
                tokio::select! {
                    msg = router_sock.recv() => {
                        let Some(Ok(frames)) = msg else { break };
                        if let Some(reply) = handle_control(frames, &mut registry, &channel_tx) {
                            if let Err(e) = router_sock.send(reply).await {
                                error!("ZMQ ROUTER send error: {e}");
                            }
                        }
                    }
                    event = event_rx.recv() => match event {
                        Ok(EngineEvent::OrderbookSnapshot(snap)) => {
                            for (topic, bytes) in dispatch(&snap, &mut registry, Instant::now()) {
                                if let Err(e) = market_pub.send(topic, bytes).await {
                                    error!("ZMQ market PUB send error: {e}");
                                }
                            }
                        }
                        Ok(EngineEvent::OrderbookRaw(_)) => {}
                        Ok(EngineEvent::User(ev)) => {
                            if let Some((topic, bytes)) = user_frame::encode(&ev) {
                                if let Err(e) = user_pub.send(topic, bytes).await {
                                    error!("ZMQ user PUB send error: {e}");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("ZMQ server lagged, skipped {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }

            info!("ZMQ server stopped");
        })
    }
}
