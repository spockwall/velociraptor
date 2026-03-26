pub mod binance;
pub mod okx;

//pub mod exchange_connector;

use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl};
use crate::exchanges::binance::BinanceConnection;
use crate::exchanges::okx::OkxConnection;
use crate::types::ExchangeName;
use crate::types::orderbook::OrderbookMessage;
use crossbeam::channel::Sender;

pub struct ExchangeConnectorFactory {
    pub message_tx: Sender<OrderbookMessage>,
    pub config: ConnectionConfig,
}

impl ExchangeConnectorFactory {
    pub fn create_connector(&self, system_control: SystemControl) -> Box<dyn ConnectionTrait> {
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
