use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
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
        let exchange = config.exchange.clone();
        let message_parser = BinanceMessageParser::with_exchange(exchange.clone());

        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            exchange,
            None,
        );

        Self { inner }
    }
}

#[async_trait]
impl ClientTrait for BinanceClient {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }
}
