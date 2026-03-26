use crate::connection::ConnectionConfig;
use anyhow::Result;
use async_trait::async_trait;

/// Trait for exchange-specific connectors
#[async_trait]
pub trait ConnectionTrait: Send + Sync {
    /// Start the connector and run until stopped
    async fn run(&mut self) -> Result<()>;

    /// Get the exchange configuration
    fn get_exchange_config(&self) -> &ConnectionConfig;
}

pub trait BasicConnectionMsgTrait {
    fn connected() -> Self;
    fn disconnected() -> Self;
    fn ping() -> Self;
    fn pong() -> Self;
    fn error(error: String) -> Self;
}

/// Trait for exchange-specific message parsers
pub trait MessageParserTrait<M: BasicConnectionMsgTrait>: Send + Sync {
    /// Parse a raw message from the exchange into our standard format
    fn parse_message(&self, text: &str) -> Result<Vec<M>>;

    /// Build ping message if exchange has custom ping format
    fn build_ping(&self) -> Option<String> {
        Some("ping".to_string())
    }

    /// Handle pong message if exchange has custom pong format
    fn is_pong(&self, text: &str) -> bool {
        text.contains("pong") // Default simple text pong
    }

    /// Check if this is a ping message requiring response
    fn is_ping(&self, text: &str) -> bool {
        text.contains("ping") // Default simple text ping
    }
}
