use crate::connection::SystemControl;
use crate::orderbook::Orderbook;
use crate::types::orderbook::OrderbookMessage;
use crossbeam::channel::Receiver;
use std::sync::Arc;
use std::thread;
use tracing::{info, warn};

/// Orderbook manager that handles multiple symbols and exchanges
pub struct OrderbookManager {
    orderbooks: Arc<dashmap::DashMap<String, Orderbook>>,
    rx: Receiver<OrderbookMessage>,
    system_control: SystemControl,
}

/// Handle to control a running OrderbookManager
pub struct OrderbookManagerHandle {
    pub handle: thread::JoinHandle<()>,
}

impl OrderbookManager {
    pub fn new(rx: Receiver<OrderbookMessage>, system_control: SystemControl) -> Self {
        Self {
            orderbooks: Arc::new(dashmap::DashMap::new()),
            rx,
            system_control,
        }
    }

    pub fn get_orderbooks(&self) -> Arc<dashmap::DashMap<String, Orderbook>> {
        self.orderbooks.clone()
    }

    pub fn start(self) -> OrderbookManagerHandle {
        let handle = thread::spawn(move || {
            info!("Orderbook manager started");

            while !self.system_control.is_shutdown() {
                match self.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(msg) => {
                        if let OrderbookMessage::OrderbookUpdate(update) = msg {
                            let key = format!("{}:{}", update.exchange, update.symbol);

                            let mut entry =
                                self.orderbooks.entry(key.clone()).or_insert_with(|| {
                                    info!("Creating new orderbook for {}", key);
                                    Orderbook::new(update.symbol.clone(), update.exchange.clone())
                                });

                            entry.apply_update(update);

                            // Log orderbook state periodically
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
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        warn!("Channel disconnected, stopping orderbook manager");
                        break;
                    }
                }
            }

            info!("Orderbook manager stopped");
        });

        OrderbookManagerHandle { handle }
    }
}
