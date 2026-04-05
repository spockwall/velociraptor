use crate::connection::SystemControl;
use crate::orderbook::Orderbook;
use crate::types::events::{OrderbookEvent, OrderbookSnapshot};
use crate::types::orderbook::OrderbookMessage;
use recorder::{RecorderEvent, RecorderSnapshot};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

/// Self-contained orderbook processing engine.
///
/// Creates its own ingest channel and event broadcast internally.
/// Callers obtain a `sender()` to push `OrderbookMessage`s in,
/// and `subscribe()` to receive `OrderbookEvent`s out.
pub struct OrderbookEngine {
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    tx: mpsc::UnboundedSender<OrderbookMessage>,
    rx: mpsc::UnboundedReceiver<OrderbookMessage>,
    event_tx: broadcast::Sender<OrderbookEvent>,
}

/// Handle to the running `OrderbookEngine` task.
pub struct OrderbookEngineHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

impl OrderbookEngine {
    pub fn new(event_broadcast_capacity: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(event_broadcast_capacity);
        Self {
            orderbooks: Arc::new(dashmap::DashMap::new()),
            tx,
            rx,
            event_tx,
        }
    }

    /// Returns a sender that connectors use to push `OrderbookMessage`s in.
    pub fn sender(&self) -> mpsc::UnboundedSender<OrderbookMessage> {
        self.tx.clone()
    }

    /// Subscribe directly to the event broadcast stream.
    pub fn subscribe(&self) -> broadcast::Receiver<OrderbookEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe and map to `RecorderEvent`, which `recorder::StorageWriter` consumes.
    /// Spawns a lightweight forwarding task; the returned receiver can be passed
    /// directly to `StorageWriter::start()`.
    pub fn subscribe_as_recorder(&self, depth: usize) -> broadcast::Receiver<RecorderEvent> {
        let (tx, rx) = broadcast::channel(1024);
        let mut src = self.event_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match src.recv().await {
                    Ok(OrderbookEvent::Snapshot(snap)) => {
                        let book = &snap.book;
                        let (bids, asks) = book.depth(depth);
                        let rec = RecorderSnapshot {
                            exchange: snap.exchange.to_string(),
                            symbol: snap.symbol.clone(),
                            sequence: snap.sequence,
                            timestamp: snap.timestamp,
                            best_bid: book.best_bid(),
                            best_ask: book.best_ask(),
                            spread: book.spread(),
                            mid: book.mid_price(),
                            wmid: book.wmid(),
                            bids,
                            asks,
                        };
                        if tx.send(RecorderEvent::Snapshot(rec)).is_err() {
                            break; // all receivers dropped
                        }
                    }
                    Ok(OrderbookEvent::RawUpdate(_)) => {
                        let _ = tx.send(RecorderEvent::RawUpdate);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        rx
    }

    /// Returns a shared handle to the live orderbook map.
    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    /// Consume the engine, spawn the processing task, and return a handle.
    pub fn start(self, system_control: SystemControl) -> OrderbookEngineHandle {
        let orderbooks = self.orderbooks;
        let mut rx = self.rx;
        let event_tx = self.event_tx;

        let handle = tokio::spawn(async move {
            info!("Orderbook engine started");

            loop {
                if system_control.is_shutdown() {
                    break;
                }

                match rx.recv().await {
                    Some(msg) => {
                        if let OrderbookMessage::OrderbookUpdate(update) = msg {
                            let key = format!("{}:{}", update.exchange, update.symbol);

                            // Emit raw update BEFORE apply
                            let _ = event_tx.send(OrderbookEvent::RawUpdate(update.clone()));

                            // Get or create the orderbook entry
                            let mut entry = orderbooks.entry(key.clone()).or_insert_with(|| {
                                info!("Creating new orderbook for {key}");
                                Arc::new(Orderbook::new(
                                    update.symbol.clone(),
                                    update.exchange.clone(),
                                ))
                            });

                            // Apply update — get a mutable Orderbook via Arc::make_mut
                            Arc::make_mut(&mut entry).apply_update(update);

                            // Log periodically
                            if entry.sequence % 100 == 0 {
                                if let (Some(bid), Some(ask)) = (entry.best_bid(), entry.best_ask())
                                {
                                    info!(
                                        "{} - Bid: {:.2} x {:.2}, Ask: {:.2} x {:.2}, Spread: {:.2}",
                                        key,
                                        bid.0,
                                        bid.1,
                                        ask.0,
                                        ask.1,
                                        ask.0 - bid.0
                                    );
                                }
                            }

                            // Emit snapshot AFTER apply
                            let snapshot = OrderbookSnapshot {
                                exchange: entry.exchange.clone(),
                                symbol: entry.symbol.clone(),
                                sequence: entry.sequence,
                                timestamp: entry.last_update,
                                book: Arc::clone(&entry),
                            };
                            let _ = event_tx.send(OrderbookEvent::Snapshot(snapshot));
                        }
                    }
                    None => {
                        warn!("Channel closed, stopping orderbook engine");
                        break;
                    }
                }
            }

            info!("Orderbook engine stopped");
        });

        OrderbookEngineHandle { handle }
    }
}
