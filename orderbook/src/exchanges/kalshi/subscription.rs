/// Builder for Kalshi orderbook subscription messages.
///
/// Kalshi accepts multiple tickers in a single subscribe command by listing
/// multiple channels. Each ticker generates one channel string of the form
/// `"orderbook_delta:<ticker>"`.
///
/// The resulting JSON is sent as a single text frame on connect.
///
/// # Example
/// ```
/// use orderbook::KalshiSubMsgBuilder;
/// let msg = KalshiSubMsgBuilder::new()
///     .with_ticker("FED-23DEC-T3.00")
///     .build();
/// // {"id":1,"cmd":"subscribe","params":{"channels":["orderbook_delta:FED-23DEC-T3.00"]}}
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
        let channels: Vec<String> = self
            .tickers
            .iter()
            .map(|t| format!("orderbook_delta:{t}"))
            .collect();

        serde_json::json!({
            "id": self.cmd_id,
            "cmd": "subscribe",
            "params": { "channels": channels }
        })
        .to_string()
    }
}

impl Default for KalshiSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
