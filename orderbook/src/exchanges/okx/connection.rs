use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::okx::OkxMessageParser;
use crate::types::ExchangeName;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct OkxConnection {
    inner: ConnectionBase<OkxMessageParser, OrderbookMessage>,
}

impl OkxConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = OkxMessageParser::new();

        // OKX doesn't need any special pre/post subscription messages
        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Okx,
            vec![], // No pre-subscription messages
            vec![], // No post-subscription messages
        );

        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for OkxConnection {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ConnectionConfig {
        self.inner.get_exchange_config()
    }
}
