use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::polymarket::{PolymarketChannel, PolymarketMessageParser};
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
        // Pick the right parser channel based on the WS URL the caller set.
        // The user channel needs application-level "PING" keepalive; the
        // market channel doesn't. Getting this wrong is what caused the
        // user-channel disconnect-loop bug.
        let channel = if config.ws_url.contains("/ws/user") {
            PolymarketChannel::User
        } else {
            PolymarketChannel::Market
        };
        let message_parser = PolymarketMessageParser::for_channel(channel);

        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            message_parser,
            ExchangeName::Polymarket,
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
