use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::ExchangeName;
use crate::exchanges::kalshi::KalshiMessageParser;
use crate::exchanges::kalshi::auth::KalshiAuth;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

/// WebSocket connection to Kalshi's orderbook feed.
///
/// **Authentication**: Kalshi requires RSA-PSS signed HTTP headers on the WebSocket
/// upgrade request. When `with_auth=true`, credentials must be supplied via
/// `ConnectionConfig::set_api_credentials(key_id, pem, None)`:
/// - `api_key`    — API key UUID (sent as `KALSHI-ACCESS-KEY` header)
/// - `api_secret` — RSA private key PEM (PKCS#8) used to sign each request
///
/// Get a key pair at: https://kalshi.com/account/profile/api-keys
pub struct KalshiConnection {
    inner: ConnectionBase<KalshiMessageParser, OrderbookMessage>,
}

impl KalshiConnection {
    /// Build a Kalshi connection.
    ///
    /// `with_auth` controls whether to sign upgrade requests with the configured
    /// credentials. Set `false` only for public feeds that don't require auth.
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
        with_auth: bool,
    ) -> Self {
        let auth_header = if with_auth {
            match (&config.api_key, &config.api_secret) {
                (Some(kid), Some(pem)) => match KalshiAuth::new(kid.clone(), pem) {
                    Ok(auth) => Some(auth.build_headers()),
                    Err(e) => {
                        warn!("Kalshi: failed to build signer: {e}");
                        None
                    }
                },
                _ => {
                    warn!("Kalshi: auth requested but no credentials supplied — will likely 401");
                    None
                }
            }
        } else {
            None
        };

        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            KalshiMessageParser::new(),
            ExchangeName::Kalshi,
            vec![],
            vec![],
            auth_header,
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
