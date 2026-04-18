pub mod binance;
pub mod hyperliquid;
pub mod kalshi;
pub mod okx;
pub mod polymarket;

use crate::connection::{ClientConfig, ConnectionTrait, SystemControl};
use crate::exchanges::binance::{BinanceClient, BinanceSubMsgBuilder};
use crate::exchanges::hyperliquid::{HyperliquidClient, HyperliquidSubMsgBuilder};
use crate::exchanges::kalshi::{KalshiClient, KalshiSubMsgBuilder};
use crate::exchanges::okx::{OkxClient, OkxSubMsgBuilder};
use crate::exchanges::polymarket::{PolymarketClient, PolymarketSubMsgBuilder};
use crate::types::events::{ChannelRequest, ChannelSpawnError, ChannelSpawner};
use crate::types::orderbook::StreamMessage;
use libs::protocol::ExchangeName;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{error, info, warn};

pub struct ConnectionFactory {
    pub message_tx: UnboundedSender<StreamMessage>,
    pub config: ClientConfig,
}

impl ConnectionFactory {
    pub fn create_connection(&self, system_control: SystemControl) -> Box<dyn ConnectionTrait> {
        match self.config.exchange {
            ExchangeName::Binance => Box::new(BinanceClient::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Okx => Box::new(OkxClient::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Polymarket => Box::new(PolymarketClient::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Hyperliquid => Box::new(HyperliquidClient::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Kalshi => Box::new(KalshiClient::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
                true,
            )),
        }
    }
}

/// Build a `ClientConfig` for a single symbol on the given exchange,
/// using each exchange's default subscribe-message builder.
fn build_connection_config(req: &ChannelRequest) -> Result<ClientConfig, ChannelSpawnError> {
    let exchange = ExchangeName::from_str(&req.exchange)
        .ok_or_else(|| ChannelSpawnError::UnknownExchange(req.exchange.clone()))?;

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

    Ok(ClientConfig::new(exchange).set_subscription_message(sub_msg))
}

/// Spawn a reconnecting connector task for one exchange config.
pub fn spawn_connection(
    exchange_config: ClientConfig,
    message_tx: mpsc::UnboundedSender<StreamMessage>,
    system_control: SystemControl,
) -> tokio::task::JoinHandle<()> {
    let exchange_name = exchange_config.exchange.clone();

    info!("Starting connection for {exchange_name}");

    tokio::spawn(async move {
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
    })
}

/// Default `ChannelSpawner` — dispatches on `ExchangeName`, builds a
/// single-symbol subscribe message via each exchange's default builder, and
/// delegates to `spawn_connection` for the reconnecting task loop.
pub struct DefaultChannelSpawner;

impl ChannelSpawner for DefaultChannelSpawner {
    fn spawn(
        &self,
        req: ChannelRequest,
        message_tx: mpsc::UnboundedSender<StreamMessage>,
        ctrl: SystemControl,
    ) -> Result<tokio::task::JoinHandle<()>, ChannelSpawnError> {
        let cfg = build_connection_config(&req)?;
        Ok(spawn_connection(cfg, message_tx, ctrl))
    }
}
