pub mod error;
pub mod orderbook_system;

use crate::connection::ConnectionConfig;
pub use crate::orderbook::Orderbook;
pub use error::*;
pub use orderbook_system::OrderbookSystem;
use std::time::Duration;

/// Main configuration and builder for the orderbook system.
#[derive(Clone, Debug)]
pub struct OrderbookSystemConfig {
    pub exchanges: Vec<ConnectionConfig>,
    /// Print orderbook stats on this interval. None = disabled.
    pub stats_interval: Option<Duration>,
    /// Capacity of the broadcast channel for OrderbookEvent. Default: 1024.
    pub event_broadcast_capacity: usize,
}

impl Default for OrderbookSystemConfig {
    fn default() -> Self {
        Self {
            exchanges: Vec::new(),
            stats_interval: Some(Duration::from_secs(10)),
            event_broadcast_capacity: 1024,
        }
    }
}

impl OrderbookSystemConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pre-built exchange config.
    pub fn with_exchange(&mut self, exchange: ConnectionConfig) -> &mut Self {
        self.exchanges.push(exchange);
        self
    }

    /// Set stats reporting interval.
    pub fn set_stats_interval(&mut self, interval: Duration) -> &mut Self {
        self.stats_interval = Some(interval);
        self
    }

    /// Disable periodic stats printing.
    pub fn disable_stats(&mut self) -> &mut Self {
        self.stats_interval = None;
        self
    }

    /// Set broadcast channel capacity (number of events buffered per subscriber).
    pub fn set_event_broadcast_capacity(&mut self, capacity: usize) -> &mut Self {
        self.event_broadcast_capacity = capacity;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> ApiResult<()> {
        if self.exchanges.is_empty() {
            return Err(ApiError::InvalidConfig("No exchanges configured".into()));
        }
        Ok(())
    }
}
