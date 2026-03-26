/// Builder for Binance combined stream subscription messages.
///
/// # Example
/// ```
/// let msg = BinanceSubMsgBuilder::new()
///     .with_depth_channel(&["btcusdt", "ethusdt"])
///     .build();
/// // {"method":"SUBSCRIBE","params":["btcusdt@depth20@100ms","ethusdt@depth20@100ms"],"id":1}
/// ```
pub struct BinanceSubMsgBuilder {
    params: Vec<String>,
}

impl BinanceSubMsgBuilder {
    pub fn new() -> Self {
        Self { params: Vec::new() }
    }

    /// Add partial depth channels for the given symbols (lowercase).
    /// Uses the `@depth20@100ms` stream (full snapshot every 100ms, 20 levels).
    pub fn with_orderbook_channel(mut self, symbols: &[&str]) -> Self {
        for sym in symbols {
            self.params
                .push(format!("{}@depth20@100ms", sym.to_lowercase()));
        }
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({
            "method": "SUBSCRIBE",
            "params": self.params,
            "id": 1
        })
        .to_string()
    }
}

impl Default for BinanceSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
