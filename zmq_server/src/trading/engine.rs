//! `TradingEngine` — unified market-data + user-event engine.
//!
//! Unlike `OrderbookEngine`, this engine demuxes both orderbook updates and
//! `UserEvent`s off the same ingest channel and broadcasts a single
//! `libs::protocol::EngineEvent` stream that transport layers (e.g.
//! `zmq_server`) can subscribe to via the `EngineEventSource` trait.
//!
//! `OrderbookEngine` is intentionally left untouched — recorder and other
//! consumers that only care about market data keep using it.

use crate::trading::events::{
    EngineEvent, EngineEventSource, OrderbookRawUpdate, OrderbookSnapshotPayload,
};
use orderbook::connection::SystemControl;
use orderbook::orderbook::Orderbook;
use orderbook::types::orderbook::OrderbookMessage;
use recorder::{RecorderEvent, RecorderSnapshot};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

/// Self-contained engine that applies orderbook updates and republishes
/// user events on a single broadcast.
pub struct TradingEngine {
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    tx: mpsc::UnboundedSender<OrderbookMessage>,
    rx: mpsc::UnboundedReceiver<OrderbookMessage>,
    event_tx: broadcast::Sender<EngineEvent>,
    /// Depth used when materializing snapshot payloads for broadcast.
    snapshot_depth: usize,
}

/// Handle to a running `TradingEngine` task.
pub struct TradingEngineHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

/// Lightweight cloneable `EngineEventSource` for sharing the broadcast sender
/// with transport consumers without pinning the `TradingEngine` behind an
/// `Arc`.
#[derive(Clone)]
pub struct TradingEngineBus {
    event_tx: broadcast::Sender<EngineEvent>,
}

impl EngineEventSource for TradingEngineBus {
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }
}

impl TradingEngine {
    pub fn new(event_broadcast_capacity: usize, snapshot_depth: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(event_broadcast_capacity);
        Self {
            orderbooks: Arc::new(dashmap::DashMap::new()),
            tx,
            rx,
            event_tx,
            snapshot_depth,
        }
    }

    /// Sender used by exchange connectors (public + user channels) to push
    /// messages into the engine.
    pub fn sender(&self) -> mpsc::UnboundedSender<OrderbookMessage> {
        self.tx.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    /// Cheap handle that implements `EngineEventSource` — hand this to
    /// `zmq_server::ZmqServer::start`.
    pub fn bus(&self) -> TradingEngineBus {
        TradingEngineBus {
            event_tx: self.event_tx.clone(),
        }
    }

    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    /// Subscribe and map to `RecorderEvent`, ready to feed `StorageWriter::start`.
    /// Snapshot payloads already carry the top-N depth the engine was configured
    /// with, so the writer receives the same record shape as today's
    /// `OrderbookEngine::subscribe_as_recorder` output.
    pub fn subscribe_as_recorder(&self) -> broadcast::Receiver<RecorderEvent> {
        let (tx, rx) = broadcast::channel(1024);
        let mut src = self.event_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match src.recv().await {
                    Ok(EngineEvent::OrderbookSnapshot(snap)) => {
                        let rec = RecorderSnapshot {
                            exchange: snap.exchange.to_string(),
                            symbol: snap.symbol,
                            sequence: snap.sequence,
                            timestamp: snap.timestamp,
                            best_bid: snap.best_bid,
                            best_ask: snap.best_ask,
                            spread: snap.spread,
                            mid: snap.mid,
                            wmid: snap.wmid,
                            bids: snap.bids,
                            asks: snap.asks,
                        };
                        if tx.send(RecorderEvent::Snapshot(rec)).is_err() {
                            break;
                        }
                    }
                    Ok(EngineEvent::OrderbookRaw(_)) => {
                        let _ = tx.send(RecorderEvent::RawUpdate);
                    }
                    Ok(EngineEvent::User(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        rx
    }

    pub fn start(self, system_control: SystemControl) -> TradingEngineHandle {
        let orderbooks = self.orderbooks;
        let mut rx = self.rx;
        let event_tx = self.event_tx;
        let depth = self.snapshot_depth;

        let handle = tokio::spawn(async move {
            info!("Trading engine started");

            loop {
                if system_control.is_shutdown() {
                    break;
                }

                match rx.recv().await {
                    Some(OrderbookMessage::OrderbookUpdate(update)) => {
                        let key = format!("{}:{}", update.exchange, update.symbol);

                        let _ = event_tx.send(EngineEvent::OrderbookRaw(OrderbookRawUpdate {
                            exchange: update.exchange.clone(),
                            symbol: update.symbol.clone(),
                            timestamp: update.timestamp,
                        }));

                        let mut entry = orderbooks.entry(key.clone()).or_insert_with(|| {
                            info!("Creating new orderbook for {key}");
                            Arc::new(Orderbook::new(
                                update.symbol.clone(),
                                update.exchange.clone(),
                            ))
                        });

                        Arc::make_mut(&mut entry).apply_update(update);

                        if entry.sequence % 100 == 0 {
                            if let (Some(bid), Some(ask)) = (entry.best_bid(), entry.best_ask()) {
                                info!(
                                    "{} - Bid: {:.4} x {:.4}, Ask: {:.4} x {:.4}, Spread: {:.4}",
                                    key,
                                    bid.0,
                                    bid.1,
                                    ask.0,
                                    ask.1,
                                    ask.0 - bid.0
                                );
                            }
                        }

                        let (bids, asks) = entry.depth(depth);
                        let payload = OrderbookSnapshotPayload {
                            exchange: entry.exchange.clone(),
                            symbol: entry.symbol.clone(),
                            sequence: entry.sequence,
                            timestamp: entry.last_update,
                            best_bid: entry.best_bid(),
                            best_ask: entry.best_ask(),
                            spread: entry.spread(),
                            mid: entry.mid_price(),
                            wmid: entry.wmid(),
                            bids,
                            asks,
                        };
                        let _ = event_tx.send(EngineEvent::OrderbookSnapshot(payload));
                    }
                    Some(OrderbookMessage::UserEvent(ev)) => {
                        let _ = event_tx.send(EngineEvent::User(ev));
                    }
                    Some(OrderbookMessage::Base(b)) => {
                        debug!("Base connection message: {:?}", b);
                    }
                    None => {
                        warn!("Channel closed, stopping trading engine");
                        break;
                    }
                }
            }

            info!("Trading engine stopped");
        });

        TradingEngineHandle { handle }
    }
}
