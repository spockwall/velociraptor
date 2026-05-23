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

/// Snapshot frame for a rolling market published on a **static topic**
/// `{exchange}:{base_slug}` (e.g. `polymarket:btc-updown-15m`) that does not
/// change across window rollover. The payload's `symbol` carries the
/// per-window asset_id and `full_slug` carries the window identity, so
/// subscribers can detect rollover from a single fixed subscription.
pub struct RollingSnapshotTopic<'a> {
    pub base_slug: &'a str,
    pub snap: &'a OrderbookSnapshot,
}

impl Topic for RollingSnapshotTopic<'_> {
    fn topic(&self) -> String {
        format!("{}:{}", self.snap.exchange.to_str(), self.base_slug)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        match rmp_serde::to_vec_named(self.snap) {
            Ok(b) => Some(b),
            Err(e) => {
                warn!("RollingSnapshot encode error: {e}");
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
            full_slug: None,
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

    #[test]
    fn rolling_snapshot_topic_uses_base_slug_and_preserves_full_slug() {
        let mut s = snap();
        s.exchange = ExchangeName::Polymarket;
        s.symbol = "0xabc-asset-id".into();
        s.full_slug = Some("btc-updown-15m-1715423400".into());
        let t = RollingSnapshotTopic {
            base_slug: "btc-updown-15m",
            snap: &s,
        };
        // Static topic — does NOT include the asset_id (which would change at rollover).
        assert_eq!(t.topic(), "polymarket:btc-updown-15m");
        let bytes = t.encode().unwrap();
        let decoded: OrderbookSnapshot = super::super::decode(&bytes).unwrap();
        assert_eq!(decoded.symbol, "0xabc-asset-id");
        assert_eq!(
            decoded.full_slug.as_deref(),
            Some("btc-updown-15m-1715423400"),
        );
    }

    #[test]
    fn decode_old_snapshot_missing_full_slug_yields_none() {
        // Encode a snapshot WITHOUT full_slug set (None) and confirm it
        // round-trips with full_slug == None — pins backward compatibility
        // for already-encoded Redis values / recorded files.
        let s = snap();
        assert!(s.full_slug.is_none());
        let bytes = rmp_serde::to_vec_named(&s).unwrap();
        let decoded: OrderbookSnapshot = super::super::decode(&bytes).unwrap();
        assert!(decoded.full_slug.is_none());
    }
}
