/// Builder for Hyperliquid l2Book subscription messages.
///
/// Hyperliquid requires one subscription frame per coin symbol.
/// `build()` returns one JSON string per coin; the generic client sends each
/// as its own WebSocket frame on connect.
///
/// # Example
/// ```
/// use orderbook::HyperliquidSubMsgBuilder;
/// let msgs = HyperliquidSubMsgBuilder::new().with_coin("BTC").build();
/// // ["{\"method\":\"subscribe\",\"subscription\":{\"type\":\"l2Book\",\"coin\":\"BTC\"}}"]
/// ```
pub struct HyperliquidSubMsgBuilder {
    coins: Vec<String>,
}

impl HyperliquidSubMsgBuilder {
    pub fn new() -> Self {
        Self { coins: Vec::new() }
    }

    /// Add a single coin symbol (e.g. "BTC", "ETH"). Stored as uppercase.
    pub fn with_coin(mut self, coin: &str) -> Self {
        self.coins.push(coin.to_uppercase());
        self
    }

    /// Add multiple coin symbols at once.
    pub fn with_coins(mut self, coins: &[&str]) -> Self {
        for c in coins {
            self.coins.push(c.to_uppercase());
        }
        self
    }

    /// Produce one subscription JSON per coin.
    pub fn build(self) -> Vec<String> {
        self.coins
            .iter()
            .map(|coin| {
                serde_json::json!({
                    "method": "subscribe",
                    "subscription": { "type": "l2Book", "coin": coin }
                })
                .to_string()
            })
            .collect()
    }
}

impl Default for HyperliquidSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
