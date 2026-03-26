pub mod error;
pub mod orderbook_system;

use crate::connection::ConnectionConfig;
pub use crate::orderbook::Orderbook;
use crate::types::ExchangeName;
pub use error::*;
pub use orderbook_system::OrderbookSystem;
use std::time::Duration;

/// Main configuration and builder for the orderbook system
#[derive(Clone, Debug)]
pub struct OrderbookSystemConfig {
    pub exchanges: Vec<ConnectionConfig>,
    pub channel_buffer_size: usize,
    pub stats_interval: Duration,
}

impl Default for OrderbookSystemConfig {
    fn default() -> Self {
        Self {
            exchanges: Vec::new(),
            channel_buffer_size: 100_000,
            stats_interval: Duration::from_secs(10),
        }
    }
}

impl OrderbookSystemConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an exchange using a builder pattern
    pub fn add_exchange<F>(&mut self, f: F, name: ExchangeName) -> &mut Self
    where
        F: FnOnce(ConnectionConfig) -> ConnectionConfig,
    {
        self.exchanges.push(f(ConnectionConfig::new(name)));
        self
    }

    /// Add a pre-built exchange config
    pub fn with_exchange(&mut self, exchange: ConnectionConfig) -> &mut Self {
        self.exchanges.push(exchange);
        self
    }

    /// Add BitMEX with default settings
    //pub fn with_bitmex(&mut self, symbols: Vec<String>) -> &mut Self {
    //    self.add_exchange(|b| b.add_topics(symbols), ExchangeName::Bitmex)
    //}

    /// Add OKX with default settings
    //pub fn with_okx(&mut self, symbols: Vec<String>) -> &mut Self {
    //    self.add_exchange(|b| b.add_topics(symbols), ExchangeName::Okx)
    //}

    /// Add BitMart with default settings
    //pub fn with_bitmart(&mut self, symbols: Vec<String>) -> &mut Self {
    //    self.add_exchange(|b| b.add_topics(symbols), ExchangeName::Bitmart)
    //}

    /// Set channel buffer size
    pub fn set_channel_buffer_size(&mut self, size: usize) -> &mut Self {
        self.channel_buffer_size = size;
        self
    }

    /// Set stats reporting interval
    pub fn set_stats_interval(&mut self, interval: Duration) -> &mut Self {
        self.stats_interval = interval;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> ApiResult<()> {
        if self.exchanges.is_empty() {
            return Err(ApiError::InvalidConfig("No exchanges configured".into()));
        }
        Ok(())
    }
}
