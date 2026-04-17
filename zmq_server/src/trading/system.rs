//! `TradingSystem` — sibling to `OrderbookSystem`, wraps `TradingEngine`.
//!
//! Orchestrates exchange connectors (public + user channels) and hands the
//! engine's event broadcast to external transport layers (e.g.
//! `zmq_server::ZmqServer`) via the `EngineEventSource` trait.

use crate::trading::events::ChannelRequest;
use crate::trading::{TradingEngine, TradingEngineBus, TradingEngineHandle};
use futures_util::StreamExt;
use libs::protocol::ExchangeName;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::ConnectionFactory;
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::kalshi::KalshiSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::exchanges::polymarket::PolymarketSubMsgBuilder;
use orderbook::orderbook::Orderbook;
use orderbook::types::errors::{ApiError, ApiResult};
use orderbook::types::orderbook::OrderbookMessage;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub struct TradingSystemConfig {
    pub exchanges: Vec<ConnectionConfig>,
    /// Capacity of the broadcast channel for `EngineEvent`. Default: 1024.
    pub event_broadcast_capacity: usize,
    /// Depth used when materializing snapshot payloads. Default: 10.
    pub snapshot_depth: usize,
}

impl Default for TradingSystemConfig {
    fn default() -> Self {
        Self {
            exchanges: Vec::new(),
            event_broadcast_capacity: 1024,
            snapshot_depth: 10,
        }
    }
}

impl TradingSystemConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_exchange(&mut self, exchange: ConnectionConfig) -> &mut Self {
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

pub struct TradingSystem {
    config: TradingSystemConfig,
    engine: Option<TradingEngine>,
    handles: SystemHandles,
    system_control: SystemControl,
    channel_tx: mpsc::UnboundedSender<ChannelRequest>,
    channel_rx: Option<mpsc::UnboundedReceiver<ChannelRequest>>,
}

struct SystemHandles {
    exchange_handles: Vec<tokio::task::JoinHandle<()>>,
    engine_handle: Option<TradingEngineHandle>,
    extra_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl TradingSystem {
    pub fn new(config: TradingSystemConfig, system_control: SystemControl) -> ApiResult<Self> {
        let engine =
            TradingEngine::new(config.event_broadcast_capacity, config.snapshot_depth);
        let (channel_tx, channel_rx) = mpsc::unbounded_channel();

        Ok(Self {
            config,
            engine: Some(engine),
            handles: SystemHandles {
                exchange_handles: Vec::new(),
                engine_handle: None,
                extra_handles: Vec::new(),
            },
            system_control,
            channel_tx,
            channel_rx: Some(channel_rx),
        })
    }

    pub fn channel_request_sender(&self) -> mpsc::UnboundedSender<ChannelRequest> {
        self.channel_tx.clone()
    }

    /// Cloneable `EngineEventSource` — hand this to `ZmqServer::start`.
    pub fn engine_bus(&self) -> TradingEngineBus {
        self.engine
            .as_ref()
            .expect("engine_bus called after run()")
            .bus()
    }

    pub fn engine(&self) -> &TradingEngine {
        self.engine.as_ref().expect("engine() called after run()")
    }

    pub fn attach_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handles.extra_handles.push(handle);
    }

    pub fn orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.engine
            .as_ref()
            .expect("orderbooks called after run()")
            .orderbooks()
    }

    async fn shutdown(self) {
        info!("Shutting down trading system...");
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
        info!("Trading system shutdown complete");
    }

    pub async fn run(mut self) -> ApiResult<()> {
        info!("=== Trading System Configuration ===");
        info!("Exchanges: {}", self.config.exchanges.len());
        info!(
            "Broadcast capacity: {}",
            self.config.event_broadcast_capacity
        );
        info!("Snapshot depth: {}", self.config.snapshot_depth);

        let engine = self.engine.take().expect("run() called twice");
        let tx = engine.sender();

        let engine_handle = engine.start(self.system_control.clone());
        self.handles.engine_handle = Some(engine_handle);

        for exchange_config in self.config.exchanges.clone() {
            let handle =
                spawn_connection(exchange_config, tx.clone(), self.system_control.clone()).await?;
            self.handles.exchange_handles.push(handle);
        }

        let exchange_handles = std::mem::take(&mut self.handles.exchange_handles);
        let system_control = self.system_control.clone();
        let mut channel_rx = self.channel_rx.take().expect("run() called twice");

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
            _ = async {
                while let Some(req) = channel_rx.recv().await {
                    info!(
                        "Dynamic channel request: {}:{} (from client {:?})",
                        req.exchange, req.symbol, req.client_id
                    );
                    match build_connection_config(&req) {
                        Ok(cfg) => {
                            match spawn_connection(cfg, tx.clone(), system_control.clone()).await {
                                Ok(_handle) => {
                                    info!("Started new channel {}:{}", req.exchange, req.symbol);
                                }
                                Err(e) => error!(
                                    "Failed to start channel {}:{}: {e}",
                                    req.exchange, req.symbol
                                ),
                            }
                        }
                        Err(e) => error!(
                            "Cannot build config for {}:{}: {e}",
                            req.exchange, req.symbol
                        ),
                    }
                }
            } => {}
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

fn build_connection_config(req: &ChannelRequest) -> Result<ConnectionConfig, String> {
    let exchange = ExchangeName::from_str(&req.exchange)
        .ok_or_else(|| format!("unknown exchange '{}'", req.exchange))?;

    let sub_msg = match exchange {
        ExchangeName::Binance => BinanceSubMsgBuilder::new()
            .with_orderbook_channel(&[req.symbol.to_lowercase().as_str()])
            .build(),
        ExchangeName::Okx => OkxSubMsgBuilder::new()
            .with_orderbook_channel(&req.symbol, "SPOT")
            .build(),
        ExchangeName::Polymarket => PolymarketSubMsgBuilder::new()
            .with_asset(&req.symbol)
            .build(),
        ExchangeName::Hyperliquid => HyperliquidSubMsgBuilder::new()
            .with_coin(&req.symbol)
            .build(),
        ExchangeName::Kalshi => KalshiSubMsgBuilder::new().with_ticker(&req.symbol).build(),
    };

    Ok(ConnectionConfig::new(exchange).set_subscription_message(sub_msg))
}
