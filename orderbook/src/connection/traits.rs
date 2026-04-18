use anyhow::Result;
use async_trait::async_trait;

/// HTTP headers to inject on the WebSocket upgrade request.
/// A list of (name, value) pairs. Rebuilt per connect attempt so auth
/// timestamps/signatures stay current.
pub type AuthHeader = Vec<(String, String)>;

/// Trait for exchange-specific connections
#[async_trait]
pub trait ClientTrait: Send + Sync {
    /// Start the connection and run until stopped
    async fn run(&mut self) -> Result<()>;
}

pub trait BasicClientMsgTrait {
    fn connected() -> Self;
    fn disconnected() -> Self;
    fn ping() -> Self;
    fn pong() -> Self;
    fn error(error: String) -> Self;
}

/// Trait for exchange-specific message parsers
pub trait MsgParserTrait<M: BasicClientMsgTrait>: Send + Sync {
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
