//! `StreamSystem` — orchestrates exchange connectors and a `StreamEngine`.
//!
//! The engine is handed in already-configured (hooks registered) so the
//! system doesn't re-expose `on_*`.

use crate::connection::{ClientConfig, SystemControl};
use crate::exchanges::spawn_connection;
use crate::orderbook::{Orderbook, StreamEngine, StreamEngineBus, StreamEngineHandle};
use crate::types::errors::{ApiError, ApiResult};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Configuration for a `StreamSystem`.
#[derive(Clone)]
pub struct StreamSystemConfig {
    pub exchanges: Vec<ClientConfig>,
    /// Capacity of the broadcast channel for `StreamEvent`. Default: 1024.
    pub event_broadcast_capacity: usize,
    /// Depth used when materializing snapshot payloads. Default: 20.
    pub snapshot_depth: usize,
}

impl Default for StreamSystemConfig {
    fn default() -> Self {
        Self {
            exchanges: Vec::new(),
            event_broadcast_capacity: 1024,
            snapshot_depth: 20,
        }
    }
}

impl StreamSystemConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_exchange(&mut self, exchange: ClientConfig) -> &mut Self {
        self.exchanges.push(exchange);
        self
    }

    pub fn set_event_broadcast_capacity(&mut self, capacity: usize) -> &mut Self {
        self.event_broadcast_capacity = capacity;
        self
    }

    pub fn set_snapshot_depth(&mut self, depth: usize) -> &mut Self {
        self.snapshot_depth = depth;
        self
    }

    pub fn validate(&self) -> ApiResult<()> {
        if self.exchanges.is_empty() {
            return Err(ApiError::InvalidConfig("No exchanges configured".into()));
        }
        Ok(())
    }
}

pub struct StreamSystem {
    config: StreamSystemConfig,
    engine_bus: StreamEngineBus,
    message_tx: mpsc::UnboundedSender<crate::types::orderbook::StreamMessage>,
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    engine_handle: StreamEngineHandle,
    exchange_handles: Vec<tokio::task::JoinHandle<()>>,
    extra_handles: Vec<tokio::task::JoinHandle<()>>,
    system_control: SystemControl,
}

impl StreamSystem {
    /// Build a system from a pre-configured engine + config.
    ///
    /// Register any typed hooks on the engine **before** calling this — the
    /// engine is consumed and started here.
    pub fn new(
        engine: StreamEngine,
        config: StreamSystemConfig,
        system_control: SystemControl,
    ) -> ApiResult<Self> {
        let engine_bus = engine.bus();
        let message_tx = engine.get_message_sender();
        let orderbooks = engine.get_orderbooks();
        let engine_handle = engine.start(system_control.clone());

        Ok(Self {
            config,
            engine_bus,
            message_tx,
            orderbooks,
            engine_handle,
            exchange_handles: Vec::new(),
            extra_handles: Vec::new(),
            system_control,
        })
    }

    /// Cloneable `StreamEventSource` for transport layers.
    pub fn engine_bus(&self) -> StreamEngineBus {
        self.engine_bus.clone()
    }

    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    pub fn attach_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.extra_handles.push(handle);
    }

    async fn shutdown(self) {
        info!("Shutting down stream system...");
        self.system_control.shutdown();
        self.engine_handle.handle.abort();
        for h in self.exchange_handles {
            h.abort();
        }
        for h in self.extra_handles {
            h.abort();
        }
        info!("Stream system shutdown complete");
    }

    pub async fn run(mut self) -> ApiResult<()> {
        info!("=== Stream System Configuration ===");
        info!("Exchanges: {}", self.config.exchanges.len());
        info!(
            "Broadcast capacity: {}",
            self.config.event_broadcast_capacity
        );
        info!("Snapshot depth: {}", self.config.snapshot_depth);

        for exchange_config in self.config.exchanges.clone() {
            let handle = spawn_connection(
                exchange_config,
                self.message_tx.clone(),
                self.system_control.clone(),
            );
            self.exchange_handles.push(handle);
        }

        let exchange_handles = std::mem::take(&mut self.exchange_handles);
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
