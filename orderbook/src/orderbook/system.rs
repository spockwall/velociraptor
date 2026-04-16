use crate::connection::{ConnectionConfig, SystemControl};
use crate::exchanges::ConnectionFactory;
use crate::orderbook::{Orderbook, OrderbookEngine, OrderbookEngineHandle};
use crate::types::errors::{ApiError, ApiResult};
use crate::types::events::OrderbookEvent;
use crate::types::orderbook::OrderbookMessage;
use futures_util::StreamExt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

/// Main configuration and builder for the orderbook system.
#[derive(Clone, Debug)]
pub struct OrderbookSystemConfig {
    pub exchanges: Vec<ConnectionConfig>,
    /// Capacity of the broadcast channel for OrderbookEvent. Default: 1024.
    pub event_broadcast_capacity: usize,
}

/// Orchestrates exchange connections and the orderbook engine.
/// The engine is self-contained — `OrderbookSystem` is a thin wiring layer.
pub struct OrderbookSystem {
    config: OrderbookSystemConfig,
    engine: Option<OrderbookEngine>,
    handles: SystemHandles,
    system_control: SystemControl,
}

struct SystemHandles {
    exchange_handles: Vec<tokio::task::JoinHandle<()>>,
    engine_handle: Option<OrderbookEngineHandle>,
    extra_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl OrderbookSystem {
    pub fn new(config: OrderbookSystemConfig, system_control: SystemControl) -> ApiResult<Self> {
        let engine = OrderbookEngine::new(config.event_broadcast_capacity);

        Ok(Self {
            config,
            engine: Some(engine),
            handles: SystemHandles {
                exchange_handles: Vec::new(),
                engine_handle: None,
                extra_handles: Vec::new(),
            },
            system_control,
        })
    }

    /// Register an async callback invoked for every `OrderbookEvent`.
    /// Returns a `JoinHandle` — store it if you need to cancel the subscription later.
    /// Must be called before `run()`.
    pub fn on_update<F, Fut>(&self, handler: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(OrderbookEvent) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut rx = self
            .engine
            .as_ref()
            .expect("on_update called after run()")
            .subscribe();

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

    /// Return a reference to the underlying engine.
    /// Use this to start external subscribers (e.g. `recorder::StorageWriter`)
    /// without coupling the `orderbook` crate to the `recorder` crate.
    pub fn engine(&self) -> &OrderbookEngine {
        self.engine.as_ref().expect("engine() called after run()")
    }

    /// Attach an already-spawned task handle so it is awaited alongside the
    /// system's other handles during `run()` and shutdown.
    pub fn attach_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handles.extra_handles.push(handle);
    }

    /// Access the live orderbook map directly.
    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.engine
            .as_ref()
            .expect("orderbooks called after run()")
            .orderbooks()
    }

    async fn shutdown(self) {
        info!("Shutting down orderbook system...");
        self.system_control.shutdown();

        if let Some(h) = self.handles.engine_handle {
            h.handle.abort();
        }
        for h in self.handles.exchange_handles {
            h.abort();
        }
        for h in self.handles.extra_handles {
            h.abort();
        }
        info!("Orderbook system shutdown complete");
    }

    /// Run the system until shutdown signal or all connections finish.
    pub async fn run(mut self) -> ApiResult<()> {
        info!("=== Orderbook System Configuration ===");
        info!("Exchanges: {}", self.config.exchanges.len());
        info!(
            "Broadcast capacity: {}",
            self.config.event_broadcast_capacity
        );

        // Take the engine out before consuming self fields
        let engine = self.engine.take().expect("run() called twice");

        // Capture sender before engine is consumed by start()
        let tx = engine.sender();

        // Start the engine task
        let engine_handle = engine.start(self.system_control.clone());
        self.handles.engine_handle = Some(engine_handle);

        // Start exchange connectors
        for exchange_config in self.config.exchanges.clone() {
            let handle =
                spawn_connection(exchange_config, tx.clone(), self.system_control.clone()).await?;
            self.handles.exchange_handles.push(handle);
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

async fn spawn_connection(
    exchange_config: ConnectionConfig,
    message_tx: mpsc::UnboundedSender<OrderbookMessage>,
    system_control: SystemControl,
) -> ApiResult<tokio::task::JoinHandle<()>> {
    let exchange_name = exchange_config.exchange.clone();

    info!("Starting connection for {exchange_name}");

    let handle = tokio::spawn(async move {
        let factory = ConnectionFactory {
            config: exchange_config,
            message_tx,
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

impl Default for OrderbookSystemConfig {
    fn default() -> Self {
        Self {
            exchanges: Vec::new(),
            event_broadcast_capacity: 1024,
        }
    }
}

impl OrderbookSystemConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pre-built exchange config.
    pub fn with_exchange(&mut self, exchange: ConnectionConfig) -> &mut Self {
        self.exchanges.push(exchange);
        self
    }

    /// Set broadcast channel capacity (number of events buffered per subscriber).
    pub fn set_event_broadcast_capacity(&mut self, capacity: usize) -> &mut Self {
        self.event_broadcast_capacity = capacity;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> ApiResult<()> {
        if self.exchanges.is_empty() {
            return Err(ApiError::InvalidConfig("No exchanges configured".into()));
        }
        Ok(())
    }
}
