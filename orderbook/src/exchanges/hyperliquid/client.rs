use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
use crate::exchanges::hyperliquid::HyperliquidMessageParser;
use crate::types::orderbook::StreamMessage;
use anyhow::Result;
use async_trait::async_trait;
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;

pub struct HyperliquidClient {
    inner: ClientBase<HyperliquidMessageParser, StreamMessage>,
}

impl HyperliquidClient {
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<StreamMessage>,
        system_control: SystemControl,
    ) -> Self {
        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            HyperliquidMessageParser::new(),
            ExchangeName::Hyperliquid,
            None,
        );

        Self { inner }
    }
}

#[async_trait]
impl ClientTrait for HyperliquidClient {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }
}
