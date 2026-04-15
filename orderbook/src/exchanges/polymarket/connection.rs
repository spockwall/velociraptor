use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::polymarket::PolymarketMessageParser;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct PolymarketConnection {
    inner: ConnectionBase<PolymarketMessageParser, OrderbookMessage>,
}

impl PolymarketConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = PolymarketMessageParser::new();

        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Polymarket,
            vec![],
            vec![],
            None,
        );

        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for PolymarketConnection {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ConnectionConfig {
        self.inner.get_exchange_config()
    }
}
