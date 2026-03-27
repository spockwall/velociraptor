use crate::api::{ApiResult, OrderbookSystemConfig};
use crate::connection::{ConnectionConfig, SystemControl};
use crate::exchanges::ConnectionFactory;
use crate::orderbook::{Orderbook, OrderbookManager};
use crate::types::events::OrderbookEvent;
use crate::types::orderbook::OrderbookMessage;
use futures_util::StreamExt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

/// Orchestrates exchange connections, the orderbook manager, and event broadcast.
pub struct OrderbookSystem {
    config: OrderbookSystemConfig,
    message_tx: mpsc::UnboundedSender<OrderbookMessage>,
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    handles: SystemHandles,
    system_control: SystemControl,
    event_tx: broadcast::Sender<OrderbookEvent>,
}

struct SystemHandles {
    exchange_handles: Vec<tokio::task::JoinHandle<()>>,
    orderbook_handle: Option<crate::orderbook::OrderbookManagerHandle>,
    stats_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OrderbookSystem {
    pub fn new(config: OrderbookSystemConfig, system_control: SystemControl) -> ApiResult<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(config.event_broadcast_capacity);

        let manager = OrderbookManager::new(message_rx, system_control.clone(), event_tx.clone());
        let orderbooks = manager.get_orderbooks();
        let orderbook_handle = manager.start();

        Ok(Self {
            config,
            message_tx,
            orderbooks,
            handles: SystemHandles {
                exchange_handles: Vec::new(),
                orderbook_handle: Some(orderbook_handle),
                stats_handle: None,
            },
            system_control,
            event_tx,
        })
    }

    /// Register an async callback invoked for every OrderbookEvent.
    /// Returns a JoinHandle — store it if you need to cancel the subscription later.
    /// Must be called before run().
    pub fn on_update<F, Fut>(&self, handler: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(OrderbookEvent) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut rx = self.event_tx.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => handler(event).await,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("on_update handler lagged, skipped {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    /// Access the live orderbook map directly.
    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    async fn start_connections(&mut self) -> ApiResult<()> {
        for exchange_config in self.config.exchanges.clone() {
            let handle = self.spawn_connection(exchange_config).await?;
            self.handles.exchange_handles.push(handle);
        }
        Ok(())
    }

    async fn spawn_connection(
        &self,
        exchange_config: ConnectionConfig,
    ) -> ApiResult<tokio::task::JoinHandle<()>> {
        let tx = self.message_tx.clone();
        let exchange_name = exchange_config.exchange.clone();
        let system_control = self.system_control.clone();

        info!("Starting connection for {exchange_name}");

        let handle = tokio::spawn(async move {
            let factory = ConnectionFactory {
                config: exchange_config,
                message_tx: tx,
            };

            loop {
                if system_control.is_shutdown() {
                    info!("{exchange_name} connection shutdown requested");
                    break;
                }

                let mut connection = factory.create_connection(system_control.clone());

                match connection.run().await {
                    Ok(_) => {
                        info!("{exchange_name} connection finished normally");
                        break;
                    }
                    Err(e) => {
                        error!("{exchange_name} connection error: {e}");

                        if system_control.is_shutdown() {
                            info!("{exchange_name} shutdown during retry");
                            break;
                        }

                        warn!("{exchange_name} will retry in 30 seconds");
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            }
        });

        Ok(handle)
    }

    fn spawn_stats_reporter(
        orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
        interval: Duration,
        system_control: SystemControl,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if system_control.is_shutdown() {
                    break;
                }
                Self::print_orderbook_stats(&orderbooks);
            }
        })
    }

    fn print_orderbook_stats(orderbooks: &dashmap::DashMap<String, Arc<Orderbook>>) {
        println!("\n=== Orderbook Statistics ===");
        println!(
            "{:<20} {:>13} {:>13} {:>11} {:>12} {:>12} {:>10} {:>10}",
            "Symbol", "Best Bid", "Best Ask", "Spread", "Mid", "BBA-Qty", "Bid Depth", "Ask Depth"
        );
        println!("{}", "-".repeat(101));

        let mut entries: Vec<_> = orderbooks
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        entries.sort_by_key(|(key, _)| key.clone());

        for (key, orderbook) in entries {
            if !orderbook.is_initialized {
                println!("{:<20} {:>13}", key, "Waiting for snapshot...");
                continue;
            }

            let bid_depth = orderbook.bid_levels.len();
            let ask_depth = orderbook.ask_levels.len();

            match (orderbook.best_bid(), orderbook.best_ask()) {
                (Some((bid_price, bid_qty)), Some((ask_price, ask_qty))) => {
                    let spread = ask_price - bid_price;
                    let mid = (bid_price + ask_price) / 2.0;
                    let bba_qty = bid_qty + ask_qty;
                    println!(
                        "{:<20} {:>13.5} {:>13.5} {:>11.5} {:>12.5} {:>12.2} {:>10} {:>10}",
                        key, bid_price, ask_price, spread, mid, bba_qty, bid_depth, ask_depth
                    );
                }
                _ => {
                    println!(
                        "{:<20} {:>13} {:>13} {:>11} {:>12} {:>12} {:>10} {:>10}",
                        key, "N/A", "N/A", "N/A", "N/A", "N/A", bid_depth, ask_depth
                    );
                }
            }
        }
        println!("Total orderbooks tracked: {}", orderbooks.len());
    }

    async fn shutdown(self) {
        info!("Shutting down orderbook system...");
        self.system_control.shutdown();
        drop(self.message_tx);

        if let Some(h) = self.handles.orderbook_handle {
            h.handle.abort();
        }
        if let Some(h) = self.handles.stats_handle {
            h.abort();
        }
        for h in self.handles.exchange_handles {
            h.abort();
        }
        info!("Orderbook system shutdown complete");
    }

    /// Run the system until shutdown signal or all connections finish.
    pub async fn run(mut self) -> ApiResult<()> {
        info!("=== Orderbook System Configuration ===");
        info!("Exchanges: {}", self.config.exchanges.len());
        info!("Stats interval: {:?}", self.config.stats_interval);
        info!(
            "Broadcast capacity: {}",
            self.config.event_broadcast_capacity
        );

        self.start_connections().await?;

        if let Some(interval) = self.config.stats_interval {
            let handle = Self::spawn_stats_reporter(
                self.orderbooks.clone(),
                interval,
                self.system_control.clone(),
            );
            self.handles.stats_handle = Some(handle);
        }

        let exchange_handles = std::mem::take(&mut self.handles.exchange_handles);
        let system_control = self.system_control.clone();

        tokio::select! {
            _ = async {
                loop {
                    if system_control.is_shutdown() { break; }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            } => {
                info!("Shutdown signal received");
            }
            _ = async {
                let mut handles = futures_util::stream::FuturesUnordered::new();
                for handle in exchange_handles {
                    handles.push(handle);
                }
                while let Some(result) = handles.next().await {
                    if let Err(e) = result {
                        error!("Exchange connection failed: {e}");
                    }
                }
            } => {
                info!("All exchange connections completed");
            }
        }

        self.system_control.shutdown();
        self.shutdown().await;
        Ok(())
    }
}
