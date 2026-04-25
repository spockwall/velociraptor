use crate::types::endpoints::{binance, hyperliquid, kalshi, okx, polymarket};
use libs::protocol::ExchangeName;
use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub exchange: ExchangeName,
    pub ws_url: String,
    /// One or more subscription frames sent sequentially on connect.
    /// Single-frame exchanges push one string; Hyperliquid pushes one per coin.
    pub subscription_messages: Vec<String>,
    pub ping_interval: u64,   // seconds
    pub reconnect_delay: u64, // seconds
    pub max_reconnect_attempts: u32,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
}

impl ClientConfig {
    pub fn new(name: ExchangeName) -> Self {
        let (ws_url, ping_interval) = match name {
            ExchangeName::Okx => (okx::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::Binance => (binance::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::BinanceSpot => (binance::ws::SPOT_PUBLIC_STREAM, 15u64),
            ExchangeName::Polymarket => (polymarket::ws::PUBLIC_STREAM, 15u64),
            ExchangeName::Hyperliquid => (hyperliquid::ws::PUBLIC_STREAM, 45u64),

            // Kalshi sends server pings every 10s; client ping interval unused
            // but set conservatively to match their documented 10s window.
            ExchangeName::Kalshi => (kalshi::ws::PUBLIC_STREAM, 10u64),
        };

        Self {
            exchange: name,
            ws_url: ws_url.to_string(),
            subscription_messages: Vec::new(),
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

    /// Set a single subscription frame. Replaces any previously configured frames.
    pub fn set_subscription_message(mut self, message: String) -> Self {
        self.subscription_messages = vec![message];
        self
    }

    /// Set multiple subscription frames, sent sequentially on connect.
    pub fn set_subscription_messages(mut self, messages: Vec<String>) -> Self {
        self.subscription_messages = messages;
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

    pub fn set_api_credentials(
        mut self,
        api_key: String,
        api_secret: String,
        passphrase: Option<String>,
    ) -> Self {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
        self.passphrase = passphrase;
        self
    }

    pub fn build(self) -> Result<ClientConfig, String> {
        if self.subscription_messages.is_empty() {
            panic!("No subscription messages configured");
        }

        Ok(ClientConfig {
            exchange: self.exchange,
            ws_url: self.ws_url,
            subscription_messages: self.subscription_messages,
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
