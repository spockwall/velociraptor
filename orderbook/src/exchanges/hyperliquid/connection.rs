use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::hyperliquid::HyperliquidMessageParser;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;

pub struct HyperliquidConnection {
    inner: ConnectionBase<HyperliquidMessageParser, OrderbookMessage>,
}

impl HyperliquidConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        // HyperliquidSubMsgBuilder produces newline-delimited JSON when multiple
        // coins are requested. ConnectionBase only sends one subscription_message,
        // so split here: first line → subscription_message, rest → post_subscription_messages.
        let (first, rest) = split_subscription_messages(&config.subscription_message);
        let config = config.set_subscription_message(first);

        let inner = ConnectionBase::new(
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
impl ConnectionTrait for HyperliquidConnection {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ConnectionConfig {
        self.inner.get_exchange_config()
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
