pub mod binance;
pub mod okx;

use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl};
use crate::exchanges::binance::BinanceConnection;
use crate::exchanges::okx::OkxConnection;
use crate::types::ExchangeName;
use crate::types::orderbook::OrderbookMessage;
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
        }
    }
}
