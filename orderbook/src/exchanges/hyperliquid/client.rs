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
        // HyperliquidSubMsgBuilder produces newline-delimited JSON when multiple
        // coins are requested. ConnectionBase only sends one subscription_message,
        // so split here: first line → subscription_message, rest → post_subscription_messages.
        let (first, rest) = split_subscription_messages(&config.subscription_message);
        let config = config.set_subscription_message(first);

        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            HyperliquidMessageParser::new(),
            ExchangeName::Hyperliquid,
            vec![], // no pre-subscription messages
            rest,   // remaining coin subscriptions
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

fn split_subscription_messages(raw: &str) -> (String, Vec<String>) {
    let mut lines: Vec<String> = raw
        .split('\n')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if lines.is_empty() {
        return (String::new(), vec![]);
    }

    let first = lines.remove(0);
    (first, lines)
}
