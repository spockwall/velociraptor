use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::polymarket::PolymarketMessageParser;
use crate::types::orderbook::StreamMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct PolymarketClient {
    inner: ClientBase<PolymarketMessageParser, StreamMessage>,
}

impl PolymarketClient {
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<StreamMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = PolymarketMessageParser::new();

        let inner = ClientBase::new(
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
impl ClientTrait for PolymarketClient {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }
}
