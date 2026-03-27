use crate::connection::SystemControl;
use crate::orderbook::Orderbook;
use crate::types::events::{OrderbookEvent, OrderbookSnapshot};
use crate::types::orderbook::OrderbookMessage;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc::UnboundedReceiver};
use tracing::{info, warn};

/// Orderbook manager that handles multiple symbols and exchanges.
/// Runs as a tokio task, reads from an mpsc channel, applies updates to
/// Arc<Orderbook> entries stored in a DashMap, and broadcasts events.
pub struct OrderbookManager {
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    rx: UnboundedReceiver<OrderbookMessage>,
    system_control: SystemControl,
    event_tx: broadcast::Sender<OrderbookEvent>,
}

/// Handle to the running OrderbookManager task.
pub struct OrderbookManagerHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

impl OrderbookManager {
    pub fn new(
        rx: UnboundedReceiver<OrderbookMessage>,
        system_control: SystemControl,
        event_tx: broadcast::Sender<OrderbookEvent>,
    ) -> Self {
        Self {
            orderbooks: Arc::new(dashmap::DashMap::new()),
            rx,
            system_control,
            event_tx,
        }
    }

    pub fn get_orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    pub fn start(mut self) -> OrderbookManagerHandle {
        let handle = tokio::spawn(async move {
            info!("Orderbook manager started");

            loop {
                if self.system_control.is_shutdown() {
                    break;
                }

                match self.rx.recv().await {
                    Some(msg) => {
                        if let OrderbookMessage::OrderbookUpdate(update) = msg {
                            let key = format!("{}:{}", update.exchange, update.symbol);

                            // Emit raw update BEFORE apply
                            let _ = self
                                .event_tx
                                .send(OrderbookEvent::RawUpdate(update.clone()));

                            // Get or create the orderbook entry
                            let mut entry =
                                self.orderbooks.entry(key.clone()).or_insert_with(|| {
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
                            let _ = self.event_tx.send(OrderbookEvent::Snapshot(snapshot));
                        }
                    }
                    None => {
                        warn!("Channel closed, stopping orderbook manager");
                        break;
                    }
                }
            }

            info!("Orderbook manager stopped");
        });

        OrderbookManagerHandle { handle }
    }
}
