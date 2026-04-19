//! Market-data topics: `Snapshot` and `Bba`.
//!
//! Topic strings: `{exchange}:{symbol}` for both types.
//! Subscribers distinguish them by their subscription type, not the topic prefix.
//!
//! # Adding a new market-data type
//! 1. Add a variant to [`MarketTopic`] wrapping the new payload struct.
//! 2. Implement `topic()` and `encode()` for that variant.
//! Done — the server dispatch loop needs no changes.

use super::Topic;
use libs::protocol::OrderbookSnapshot;
use tracing::warn;

// ── Snapshot ──────────────────────────────────────────────────────────────────

/// Full orderbook snapshot frame.
pub struct SnapshotTopic<'a>(pub &'a OrderbookSnapshot);

impl Topic for SnapshotTopic<'_> {
    fn topic(&self) -> String {
        format!("{}:{}", self.0.exchange.to_str(), self.0.symbol)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        match rmp_serde::to_vec_named(self.0) {
            Ok(b) => Some(b),
            Err(e) => {
                warn!("Snapshot encode error: {e}");
                None
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
    fn snapshot_topic_and_roundtrip() {
        let s = snap();
        let t = SnapshotTopic(&s);
        assert_eq!(t.topic(), "binance:btcusdt");
        let bytes = t.encode().unwrap();
        let decoded: OrderbookSnapshot = super::super::decode(&bytes).unwrap();
        assert_eq!(decoded.symbol, "btcusdt");
        assert_eq!(decoded.sequence, 42);
    }
}
