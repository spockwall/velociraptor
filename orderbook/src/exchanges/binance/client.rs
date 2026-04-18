use crate::connection::{ClientConfig, ConnectionTrait, SystemControl, client::ClientBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::binance::BinanceMessageParser;
use crate::types::orderbook::StreamMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct BinanceClient {
    inner: ClientBase<BinanceMessageParser, StreamMessage>,
}

impl BinanceClient {
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<StreamMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = BinanceMessageParser::new();

        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Binance,
            vec![], // No pre-subscription messages
            vec![], // No post-subscription messages
            None,
        );

        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for BinanceClient {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ClientConfig {
        self.inner.get_exchange_config()
    }
}
