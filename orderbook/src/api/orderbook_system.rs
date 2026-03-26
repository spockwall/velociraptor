use crate::api::{ApiResult, OrderbookSystemConfig};
use crate::connection::{ConnectionConfig, SystemControl};
use crate::exchanges::ExchangeConnectorFactory;
use crate::orderbook::{Orderbook, OrderbookManager};
use crate::types::orderbook::OrderbookMessage;
use crossbeam::channel::{Receiver, Sender, unbounded};
use futures_util::StreamExt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

/// This is a system that instantiates the realtime orderbook that abstracts the logic of
/// connecting to the exchange server and btree map building process of orderbooks.
/// Users need to handle the shutdown signal by themselves.
/// Users need to handle the Ctrl+C signal by themselves.
///
/// It is responsible for:
/// - Instantiating the orderbook manager
/// - Instantiating the exchange connectors
/// - Starting the stats reporter
/// - Shutting down the system
pub struct OrderbookSystem {
    config: OrderbookSystemConfig,
    message_tx: Sender<OrderbookMessage>,
    orderbook_manager: Option<Arc<dashmap::DashMap<String, Orderbook>>>,
    handles: SystemHandles,
    system_control: SystemControl,
}

struct SystemHandles {
    exchange_handles: Vec<tokio::task::JoinHandle<()>>,
    orderbook_handle: Option<crate::orderbook::OrderbookManagerHandle>,
    stats_handle: Option<thread::JoinHandle<()>>,
}

impl OrderbookSystem {
    /// Create a new OrderbookSystem with the given configuration
    pub fn new(config: OrderbookSystemConfig, system_control: SystemControl) -> ApiResult<Self> {
        let (message_tx, message_rx) = unbounded();

        // Setup orderbook manager and processor
        let (orderbook_manager, orderbook_handle) =
            Self::setup_orderbook(message_rx, system_control.clone());

        let system = Self {
            config,
            message_tx,
            orderbook_manager,
            handles: SystemHandles {
                exchange_handles: Vec::new(),
                orderbook_handle,
                stats_handle: None,
            },
            system_control,
        };

        Ok(system)
    }

    fn setup_orderbook(
        message_rx: Receiver<OrderbookMessage>,
        system_control: SystemControl,
    ) -> (
        Option<Arc<dashmap::DashMap<String, Orderbook>>>,
        Option<crate::orderbook::OrderbookManagerHandle>,
    ) {
        let manager = OrderbookManager::new(message_rx, system_control);
        let orderbooks = manager.get_orderbooks();
        let orderbook_handle = manager.start();

        (Some(orderbooks), Some(orderbook_handle))
    }

    async fn start_exchange_connectors(&mut self) -> ApiResult<()> {
        for exchange_config in self.config.exchanges.clone() {
            let handle = self.spawn_exchange_connector(exchange_config).await?;
            self.handles.exchange_handles.push(handle);
        }
        Ok(())
    }

    async fn spawn_exchange_connector(
        &self,
        exchange_config: ConnectionConfig,
    ) -> ApiResult<tokio::task::JoinHandle<()>> {
        let tx = self.message_tx.clone();
        let exchange_name = exchange_config.exchange.clone();

        info!("Starting connector for {exchange_name}");

        let system_control = self.system_control.clone();
        let handle = tokio::spawn(async move {
            let factory = ExchangeConnectorFactory {
                config: exchange_config,
                message_tx: tx,
            };

            loop {
                // Check shutdown before attempting connection
                if system_control.is_shutdown() {
                    info!("{exchange_name} connector shutdown requested");
                    break;
                }

                let mut connector = factory.create_connector(system_control.clone());

                match connector.run().await {
                    Ok(_) => {
                        info!("{exchange_name} connector finished normally");
                        break;
                    }
                    Err(e) => {
                        error!("{exchange_name} connector error: {e}");

                        // Check shutdown before retrying
                        if system_control.is_shutdown() {
                            info!("{exchange_name} connector shutdown requested during retry",);
                            break;
                        }

                        warn!("{exchange_name} will retry in 30 seconds",);
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            }
        });

        Ok(handle)
    }

    fn start_stats_reporter(&mut self, orderbooks: Arc<dashmap::DashMap<String, Orderbook>>) {
        let interval = self.config.stats_interval;

        let system_control = self.system_control.clone();
        let orderbooks_clone = Arc::clone(&orderbooks);
        let handle = thread::spawn(move || {
            loop {
                // Check shutdown before sleeping
                if system_control.is_shutdown() {
                    info!("Stats reporter shutdown requested");
                    break;
                }

                thread::sleep(interval);

                OrderbookSystem::print_orderbook_stats(&orderbooks_clone);
            }
        });

        self.handles.stats_handle = Some(handle);
    }

    fn print_orderbook_stats(orderbooks: &dashmap::DashMap<String, Orderbook>) {
        println!("\n=== Orderbook Statistics ===");
        println!(
            "{:<20} {:>13} {:>13} {:>11} {:>12} {:>12} {:>10} {:>10}",
            "Symbol", "Best Bid", "Best Ask", "Spread", "Mid", "BBA-Qty", "Bid Depth", "Ask Depth"
        );
        println!("{}", "-".repeat(101));

        let mut entries: Vec<_> = orderbooks
            .iter()
            .map(|entry| (entry.key().clone(), entry))
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

    fn print_startup_info(&self) {
        info!("=== Orderbook System Configuration ===");
        info!("Exchanges: {}", self.config.exchanges.len());
        //for exchange in &self.config.exchanges {
        //    info!("  {} - {} topics", exchange.exchange, exchange.topics.len());
        //}
        info!("Stats interval: {:?}", self.config.stats_interval);
        info!("Channel buffer size: {}", self.config.channel_buffer_size);
    }

    fn shutdown(self) {
        info!("Shutting down orderbook system...");

        // Signal shutdown to all components
        self.system_control.shutdown();

        // Close the message channel to signal shutdown to all components
        drop(self.message_tx);

        // Stop orderbook manager with timeout
        if let Some(handle) = self.handles.orderbook_handle {
            let _ = handle.handle.join();
        }

        // Stop stats reporter with timeout
        if let Some(handle) = self.handles.stats_handle {
            // Give stats thread a chance to exit gracefully
            if handle.join().is_err() {
                warn!("Stats reporter thread did not exit gracefully");
            }
        }

        info!("Orderbook system shutdown complete");
    }

    pub fn get_orderbooks(&self) -> Option<Arc<dashmap::DashMap<String, Orderbook>>> {
        self.orderbook_manager.clone()
    }

    /// Run the system until stopped
    pub async fn run(mut self, tracing_status: bool) -> ApiResult<()> {
        info!("Orderbook system started");
        self.print_startup_info();

        // Start exchange connectors
        self.start_exchange_connectors().await?;

        // Start stats reporter if orderbook manager is enabled
        if tracing_status {
            if let Some(ref orderbooks) = self.orderbook_manager {
                self.start_stats_reporter(orderbooks.clone());
            }
        }

        // Wait for all exchange tasks or shutdown signal
        // Extract exchange handles from self to move into async block
        // std::mem::take() takes ownership and replaces with default (empty Vec)
        let exchange_handles = std::mem::take(&mut self.handles.exchange_handles);
        let system_control = self.system_control.clone();

        // Wait for either shutdown signal, Ctrl+C, or any exchange handle to complete
        tokio::select! {
            // Wait for shutdown signal
            _ = async {
                loop {
                    if system_control.is_shutdown() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            } => {
                info!("Shutdown signal received, stopping exchange connectors");
            }

            // Wait for any exchange handle to complete
            _ = async {
                let mut handles = futures_util::stream::FuturesUnordered::new();
                for handle in exchange_handles {
                    handles.push(handle);
                }

                while let Some(result) = handles.next().await {
                    if let Err(e) = result {
                        error!("Exchange connector failed: {:?}", e);
                    }
                }
            } => {
                info!("All exchange connectors completed");
            }
        }

        // Graceful shutdown
        self.system_control.shutdown();
        self.shutdown();
        Ok(())
    }
}
