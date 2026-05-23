use super::Topic;
use libs::protocol::LastTradePrice;
use tracing::error;

/// ZMQ topic for a public last-trade-price event.
///
/// Topic format: `"{exchange}:{symbol}:last_trade"` — e.g.
/// `"polymarket:114122...:last_trade"`.
///
/// Published on `MARKET_DATA_SOCKET` (not `WS_STATUS_SOCKET`).
pub struct LastTradeTopic<'a>(pub &'a LastTradePrice);

impl Topic for LastTradeTopic<'_> {
    fn topic(&self) -> String {
        format!("{}:{}:last_trade", self.0.exchange, self.0.symbol)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        match rmp_serde::to_vec_named(self.0) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                error!("LastTradeTopic encode error: {e}");
                None
            }
        }
    }
}

/// Rolling-market last-trade frame published on the stable topic
/// `{exchange}:{base_slug}:last_trade`. The payload's `symbol` carries the
/// per-window asset_id and `full_slug` carries the window identity, so a
/// subscriber on a fixed subscription sees rollover as a payload change.
pub struct RollingLastTradeTopic<'a> {
    pub base_slug: &'a str,
    pub trade: &'a LastTradePrice,
}

impl Topic for RollingLastTradeTopic<'_> {
    fn topic(&self) -> String {
        format!("{}:{}:last_trade", self.trade.exchange, self.base_slug)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        match rmp_serde::to_vec_named(self.trade) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                error!("RollingLastTradeTopic encode error: {e}");
                None
            }
        }
    }
}
