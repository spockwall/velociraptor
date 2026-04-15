/// Builder for Kalshi orderbook subscription messages.
///
/// Per Kalshi docs: one subscribe command names the channel once and lists
/// all market tickers in `market_tickers`. The resulting JSON is sent as a
/// single text frame on connect.
///
/// # Example
/// ```
/// use orderbook::KalshiSubMsgBuilder;
/// let msg = KalshiSubMsgBuilder::new()
///     .with_ticker("FED-23DEC-T3.00")
///     .build();
/// // {"id":1,"cmd":"subscribe","params":{"channels":["orderbook_delta"],"market_tickers":["FED-23DEC-T3.00"]}}
/// ```
pub struct KalshiSubMsgBuilder {
    tickers: Vec<String>,
    /// Subscription ID sent with the command (Kalshi echoes it in the ack).
    cmd_id: u64,
}

impl KalshiSubMsgBuilder {
    pub fn new() -> Self {
        Self {
            tickers: Vec::new(),
            cmd_id: 1,
        }
    }

    /// Add a single market ticker (e.g. `"FED-23DEC-T3.00"`).
    pub fn with_ticker(mut self, ticker: &str) -> Self {
        self.tickers.push(ticker.to_string());
        self
    }

    /// Add multiple market tickers at once.
    pub fn with_tickers(mut self, tickers: &[&str]) -> Self {
        for t in tickers {
            self.tickers.push(t.to_string());
        }
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({
            "id": self.cmd_id,
            "cmd": "subscribe",
            "params": {
                "channels": ["orderbook_delta"],
                "market_tickers": self.tickers,
            }
        })
        .to_string()
    }
}

impl Default for KalshiSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
