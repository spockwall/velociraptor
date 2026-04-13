pub mod binance;
pub mod hyperliquid;
pub mod okx;
pub mod polymarket;

use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl};
use crate::exchanges::binance::BinanceConnection;
use crate::exchanges::hyperliquid::HyperliquidConnection;
use crate::exchanges::okx::OkxConnection;
use crate::exchanges::polymarket::PolymarketConnection;
use crate::types::orderbook::OrderbookMessage;
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;

pub struct ConnectionFactory {
    pub message_tx: UnboundedSender<OrderbookMessage>,
    pub config: ConnectionConfig,
}

impl ConnectionFactory {
    pub fn create_connection(&self, system_control: SystemControl) -> Box<dyn ConnectionTrait> {
        match self.config.exchange {
            ExchangeName::Binance => Box::new(BinanceConnection::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Okx => Box::new(OkxConnection::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Polymarket => Box::new(PolymarketConnection::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Hyperliquid => Box::new(HyperliquidConnection::new(
                self.config.clone(),
                self.message_tx.clone(),
                system_control,
            )),
            ExchangeName::Kalshi => unimplemented!(),
        }
    }
}
