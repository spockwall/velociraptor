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

/// Builder for Kalshi's authenticated CF Benchmarks value feed.
///
/// # Example
/// ```
/// use orderbook::KalshiCfBenchmarksSubMsgBuilder;
/// let msg = KalshiCfBenchmarksSubMsgBuilder::new()
///     .with_indices(&["BRTI", "ETHUSD_RTI"])
///     .build();
/// // {"id":1,"cmd":"subscribe","params":{"channels":["cfbenchmarks_value"],"index_ids":["BRTI","ETHUSD_RTI"]}}
/// ```
pub struct KalshiCfBenchmarksSubMsgBuilder {
    index_ids: Vec<String>,
    cmd_id: u64,
}

impl KalshiCfBenchmarksSubMsgBuilder {
    pub fn new() -> Self {
        Self {
            index_ids: Vec::new(),
            cmd_id: 1,
        }
    }

    /// Add one CF Benchmarks index ID.
    pub fn with_index(mut self, index_id: &str) -> Self {
        self.index_ids.push(index_id.to_string());
        self
    }

    /// Add several CF Benchmarks index IDs.
    pub fn with_indices(mut self, index_ids: &[&str]) -> Self {
        self.index_ids
            .extend(index_ids.iter().map(|index_id| (*index_id).to_string()));
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({
            "id": self.cmd_id,
            "cmd": "subscribe",
            "params": {
                "channels": ["cfbenchmarks_value"],
                "index_ids": self.index_ids,
            }
        })
        .to_string()
    }
}

impl Default for KalshiCfBenchmarksSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::KalshiCfBenchmarksSubMsgBuilder;

    #[test]
    fn builds_cfbenchmarks_subscription_for_brti_and_eth_rti() {
        let msg = KalshiCfBenchmarksSubMsgBuilder::new()
            .with_indices(&["BRTI", "ETHUSD_RTI"])
            .build();
        let value: serde_json::Value = serde_json::from_str(&msg).unwrap();

        assert_eq!(value["id"], 1);
        assert_eq!(value["cmd"], "subscribe");
        assert_eq!(value["params"]["channels"][0], "cfbenchmarks_value");
        assert_eq!(value["params"]["index_ids"][0], "BRTI");
        assert_eq!(value["params"]["index_ids"][1], "ETHUSD_RTI");
        assert!(value["params"].get("market_tickers").is_none());
    }
}
