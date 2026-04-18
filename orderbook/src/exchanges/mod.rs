pub mod binance;
pub mod hyperliquid;
pub mod kalshi;
pub mod okx;
pub mod polymarket;

use crate::connection::{ClientConfig, ClientTrait, SystemControl};
use crate::exchanges::binance::BinanceClient;
use crate::exchanges::hyperliquid::HyperliquidClient;
use crate::exchanges::kalshi::KalshiClient;
use crate::exchanges::okx::OkxClient;
use crate::exchanges::polymarket::PolymarketClient;
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
    pub fn create_connection(&self, system_control: SystemControl) -> Box<dyn ClientTrait> {
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
