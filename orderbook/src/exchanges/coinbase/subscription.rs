/// Builder for Coinbase Exchange public WebSocket subscriptions.
///
/// The order-book channel uses `level2_batch`, the credential-free Level 2
/// feed. Coinbase publishes the same snapshot/update schema as `level2`, with
/// updates grouped into 50 ms batches.
pub struct CoinbaseSubMsgBuilder {
    product_ids: Vec<String>,
    channels: Vec<String>,
}

impl CoinbaseSubMsgBuilder {
    pub fn new() -> Self {
        Self {
            product_ids: Vec::new(),
            channels: Vec::new(),
        }
    }

    pub fn with_product_ids(mut self, product_ids: &[&str]) -> Self {
        for product_id in product_ids {
            let product_id = product_id.to_uppercase();
            if !self.product_ids.contains(&product_id) {
                self.product_ids.push(product_id);
            }
        }
        self
    }

    /// Subscribe to the public, 50 ms batched Level 2 book.
    pub fn with_orderbook_channel(mut self) -> Self {
        self.push_channel("level2_batch");
        self
    }

    /// Subscribe to public executions.
    pub fn with_trade_channel(mut self) -> Self {
        self.push_channel("matches");
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({
            "type": "subscribe",
            "product_ids": self.product_ids,
            "channels": self.channels,
        })
        .to_string()
    }

    fn push_channel(&mut self, channel: &str) {
        if !self.channels.iter().any(|existing| existing == channel) {
            self.channels.push(channel.to_string());
        }
    }
}

impl Default for CoinbaseSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_public_book_and_trade_subscription() {
        let raw = CoinbaseSubMsgBuilder::new()
            .with_product_ids(&["btc-usd", "ETH-USD", "BTC-USD"])
            .with_orderbook_channel()
            .with_trade_channel()
            .build();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(value["type"], "subscribe");
        assert_eq!(
            value["product_ids"],
            serde_json::json!(["BTC-USD", "ETH-USD"])
        );
        assert_eq!(
            value["channels"],
            serde_json::json!(["level2_batch", "matches"])
        );
    }
}
