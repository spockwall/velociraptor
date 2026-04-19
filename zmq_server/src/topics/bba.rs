use super::Topic;
use chrono::{DateTime, Utc};
use libs::protocol::OrderbookSnapshot;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Best-bid-ask only frame — a slim alternative to the full snapshot.
pub struct BbaTopic<'a>(pub &'a OrderbookSnapshot);

/// Owned BBA payload returned by decode.
#[derive(Debug, Serialize, Deserialize)]
pub struct BbaPayload {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
}

impl<'a> From<&'a OrderbookSnapshot> for BbaPayload {
    fn from(snap: &'a OrderbookSnapshot) -> Self {
        Self {
            exchange: snap.exchange.to_str().to_owned(),
            symbol: snap.symbol.clone(),
            sequence: snap.sequence,
            timestamp: snap.timestamp,
            best_bid: snap.best_bid,
            best_ask: snap.best_ask,
            spread: snap.spread,
        }
    }
}

impl Topic for BbaTopic<'_> {
    fn topic(&self) -> String {
        format!("{}:{}", self.0.exchange.to_str(), self.0.symbol)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let payload = BbaPayload::from(self.0);
        match rmp_serde::to_vec_named(&payload) {
            Ok(b) => Some(b),
            Err(e) => {
                warn!("BBA encode error: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::ExchangeName;

    fn snap() -> OrderbookSnapshot {
        OrderbookSnapshot {
            exchange: ExchangeName::Binance,
            symbol: "btcusdt".into(),
            sequence: 42,
            timestamp: chrono::Utc::now(),
            best_bid: Some((30000.0, 1.5)),
            best_ask: Some((30001.0, 2.0)),
            spread: Some(1.0),
            mid: Some(30000.5),
            wmid: 30000.4,
            bids: vec![(30000.0, 1.5)],
            asks: vec![(30001.0, 2.0)],
        }
    }

    #[test]
    fn bba_topic_and_roundtrip() {
        let s = snap();
        let t = BbaTopic(&s);
        assert_eq!(t.topic(), "binance:btcusdt");
        let bytes = t.encode().unwrap();
        let decoded: BbaPayload = super::super::decode(&bytes).unwrap();
        assert_eq!(decoded.symbol, "btcusdt");
        assert_eq!(decoded.best_bid, Some((30000.0, 1.5)));
    }
}
