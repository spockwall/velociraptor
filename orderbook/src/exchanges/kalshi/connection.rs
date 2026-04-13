use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::kalshi::KalshiMessageParser;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;

/// WebSocket connection to Kalshi's orderbook feed.
///
/// **Authentication**: Kalshi requires an API key even for the public orderbook
/// WebSocket. Pass it via `ConnectionConfig::api_key` — `KalshiConnection`
/// appends `?apiKey=<key>` to the WS URL automatically.
///
/// Get an API key at: https://kalshi.com/account/profile/api-keys
///
/// Without a key, the server returns HTTP 401 and no data flows.
pub struct KalshiConnection {
    inner: ConnectionBase<KalshiMessageParser, OrderbookMessage>,
}

impl KalshiConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        // Append ?apiKey=<key> to the WS URL when an API key is configured.
        let config = if let Some(ref key) = config.api_key {
            let url = if config.ws_url.contains('?') {
                format!("{}&apiKey={key}", config.ws_url)
            } else {
                format!("{}?apiKey={key}", config.ws_url)
            };
            config.set_ws_url(url)
        } else {
            config
        };

        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            KalshiMessageParser::new(),
            ExchangeName::Kalshi,
            vec![], // no pre-subscription messages
            vec![], // all tickers are in the single subscription command
        );
        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for KalshiConnection {
    async fn run(&mut self) -> Result<()> {
        self.inner.run().await
    }

    fn get_exchange_config(&self) -> &ConnectionConfig {
        self.inner.get_exchange_config()
    }
}
