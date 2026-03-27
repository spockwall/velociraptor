use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::binance::BinanceMessageParser;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct BinanceConnection {
    inner: ConnectionBase<BinanceMessageParser, OrderbookMessage>,
}

impl BinanceConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = BinanceMessageParser::new();

        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Binance,
            vec![], // No pre-subscription messages
            vec![], // No post-subscription messages
        );

        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for BinanceConnection {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ConnectionConfig {
        self.inner.get_exchange_config()
    }
}
