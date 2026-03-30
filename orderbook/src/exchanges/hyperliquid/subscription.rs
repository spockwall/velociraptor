/// Builder for Hyperliquid l2Book subscription messages.
///
/// Hyperliquid requires one subscription message per coin symbol.
/// `build()` returns newline-delimited JSON — one object per line.
/// `HyperliquidConnection` splits on `'\n'` and routes the first line to
/// `subscription_message` and the rest to `post_subscription_messages`.
///
/// # Example
/// ```
/// use orderbook::HyperliquidSubMsgBuilder;
/// let msg = HyperliquidSubMsgBuilder::new().with_coin("BTC").build();
/// // {"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}
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

    /// Produce one subscription JSON per coin, joined by newlines.
    pub fn build(self) -> String {
        self.coins
            .iter()
            .map(|coin| {
                serde_json::json!({
                    "method": "subscribe",
                    "subscription": { "type": "l2Book", "coin": coin }
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for HyperliquidSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
