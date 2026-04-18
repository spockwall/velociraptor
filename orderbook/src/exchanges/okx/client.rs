use crate::connection::{ClientConfig, ConnectionTrait, SystemControl, client::ClientBase};
use crate::exchanges::okx::OkxMessageParser;
use crate::types::orderbook::StreamMessage;
use anyhow::Result;
use async_trait::async_trait;
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;

pub struct OkxClient {
    inner: ClientBase<OkxMessageParser, StreamMessage>,
}

impl OkxClient {
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<StreamMessage>,
        system_control: SystemControl,
    ) -> Self {
        let message_parser = OkxMessageParser::new();

        // OKX doesn't need any special pre/post subscription messages
        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Okx,
            vec![], // No pre-subscription messages
            vec![], // No post-subscription messages
            None,
        );

        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for OkxClient {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ClientConfig {
        self.inner.get_exchange_config()
    }
}
