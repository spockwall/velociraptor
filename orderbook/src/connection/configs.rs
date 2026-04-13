use crate::types::endpoints::{binance, hyperliquid, kalshi, okx, polymarket};
use libs::protocol::ExchangeName;
use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub exchange: ExchangeName,
    pub ws_url: String,
    pub subscription_message: String,
    pub ping_interval: u64,   // seconds
    pub reconnect_delay: u64, // seconds
    pub max_reconnect_attempts: u32,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
}

impl ConnectionConfig {
    pub fn new(name: ExchangeName) -> Self {
        let (ws_url, ping_interval) = match name {
            ExchangeName::Okx => (okx::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::Binance => (binance::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::Polymarket => (polymarket::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::Hyperliquid => (hyperliquid::ws::PUBLIC_STREAM, 45u64),

            // Kalshi sends server pings every 10s; client ping interval unused
            // but set conservatively to match their documented 10s window.
            ExchangeName::Kalshi => (kalshi::ws::PUBLIC_STREAM, 10u64),
        };

        Self {
            exchange: name,
            ws_url: ws_url.to_string(),
            subscription_message: String::new(),
            ping_interval,
            reconnect_delay: 5,
            max_reconnect_attempts: 10,
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    pub fn set_ws_url(mut self, url: impl Into<String>) -> Self {
        self.ws_url = url.into();
        self
    }

    pub fn set_subscription_message(mut self, message: String) -> Self {
        self.subscription_message = message;
        self
    }

    pub fn set_ping_interval(mut self, seconds: u64) -> Self {
        self.ping_interval = seconds;
        self
    }

    pub fn set_reconnect_delay(mut self, seconds: u64) -> Self {
        self.reconnect_delay = seconds;
        self
    }

    pub fn set_max_reconnect_attempts(mut self, attempts: u32) -> Self {
        self.max_reconnect_attempts = attempts;
        self
    }

    /// Set only the API key (for exchanges that authenticate via a single token,
    /// e.g. Kalshi appends it as `?apiKey=<key>` to the WS URL).
    pub fn set_api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    pub fn set_api_credentials(
        mut self,
        api_key: String,
        api_secret: String,
        passphrase: String,
    ) -> Self {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
        self.passphrase = Some(passphrase);
        self
    }

    pub fn build(self) -> Result<ConnectionConfig, String> {
        if self.subscription_message.is_empty() {
            panic!("No subscription messages configured");
        }

        Ok(ConnectionConfig {
            exchange: self.exchange,
            ws_url: self.ws_url,
            subscription_message: self.subscription_message,
            ping_interval: self.ping_interval,
            reconnect_delay: self.reconnect_delay,
            max_reconnect_attempts: self.max_reconnect_attempts,
            api_key: self.api_key,
            api_secret: self.api_secret,
            passphrase: self.passphrase,
        })
    }

    pub fn random_reconnect_delay(&self) -> u64 {
        let mut rng = SmallRng::seed_from_u64(self.reconnect_delay);
        self.reconnect_delay + rng.random_range(0..50)
    }
}
