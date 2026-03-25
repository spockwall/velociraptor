use crate::core::types::{DeltaAction, OrderBookDelta};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Binance wire schema ───────────────────────────────────────────────────────

/// Raw `depthUpdate` event from the Binance futures depth stream.
///
/// ```json
/// {
///   "e": "depthUpdate",
///   "E": 123456789,
///   "T": 123456788,
///   "s": "BTCUSDT",
///   "U": 157,
///   "u": 160,
///   "pu": 149,
///   "b": [["0.0024", "10"]],
///   "a": [["0.0026", "100"]]
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinanceDepthUpdate {
    /// Event type — always `"depthUpdate"` for this stream.
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time (ms since epoch).
    #[serde(rename = "E")]
    pub event_time: i64,
    /// Transaction time (ms since epoch).
    #[serde(rename = "T")]
    pub transaction_time: i64,
    /// Symbol, e.g. `"BTCUSDT"`.
    #[serde(rename = "s")]
    pub symbol: String,
    /// First update ID in this event.
    #[serde(rename = "U")]
    pub first_update_id: i64,
    /// Final update ID in this event.
    #[serde(rename = "u")]
    pub final_update_id: i64,
    /// Final update ID of the previous stream event — used for gap detection.
    #[serde(rename = "pu")]
    pub prev_final_update_id: i64,
    /// Bid levels to update: `[price_str, qty_str]`.  qty `"0"` means remove.
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    /// Ask levels to update: `[price_str, qty_str]`.  qty `"0"` means remove.
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
}

// ── Normalizer ────────────────────────────────────────────────────────────────

fn parse_levels(raw: &[[String; 2]]) -> Vec<(f64, f64)> {
    raw.iter()
        .filter_map(|pair| Some((pair[0].parse().ok()?, pair[1].parse().ok()?)))
        .collect()
}

impl BinanceDepthUpdate {
    /// Normalize this wire message into a canonical [`OrderBookDelta`].
    pub fn normalize(self) -> OrderBookDelta {
        let mut payload = HashMap::new();
        payload.insert("bids".to_string(), parse_levels(&self.bids));
        payload.insert("asks".to_string(), parse_levels(&self.asks));

        OrderBookDelta {
            exchange: "binance".to_string(),
            symbol: self.symbol,
            action: DeltaAction::Update,
            timestamp_ms: self.transaction_time,
            sequence: self.final_update_id,
            prev_sequence: self.prev_final_update_id,
            payload,
        }
    }
}

// ── From impl (ergonomic alternative to .normalize()) ────────────────────────

impl From<BinanceDepthUpdate> for OrderBookDelta {
    fn from(msg: BinanceDepthUpdate) -> Self {
        msg.normalize()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_raw() -> &'static str {
        r#"{
            "e": "depthUpdate",
            "E": 123456789,
            "T": 123456788,
            "s": "BTCUSDT",
            "U": 157,
            "u": 160,
            "pu": 149,
            "b": [["0.0024", "10"]],
            "a": [["0.0026", "100"]]
        }"#
    }

    // ── Deserialization ───────────────────────────────────────────────────────

    #[test]
    fn deserializes_wire_message() {
        let msg: BinanceDepthUpdate = serde_json::from_str(sample_raw()).unwrap();

        assert_eq!(msg.event_type, "depthUpdate");
        assert_eq!(msg.symbol, "BTCUSDT");
        assert_eq!(msg.event_time, 123456789);
        assert_eq!(msg.transaction_time, 123456788);
        assert_eq!(msg.first_update_id, 157);
        assert_eq!(msg.final_update_id, 160);
        assert_eq!(msg.prev_final_update_id, 149);
        assert_eq!(msg.bids, vec![["0.0024".to_string(), "10".to_string()]]);
        assert_eq!(msg.asks, vec![["0.0026".to_string(), "100".to_string()]]);
    }

    // ── Normalization ─────────────────────────────────────────────────────────

    #[test]
    fn normalizes_to_order_book_delta() {
        let msg: BinanceDepthUpdate = serde_json::from_str(sample_raw()).unwrap();
        let delta = msg.normalize();

        assert_eq!(delta.exchange, "binance");
        assert_eq!(delta.symbol, "BTCUSDT");
        assert_eq!(delta.action, DeltaAction::Update);
        assert_eq!(delta.timestamp_ms, 123456788); // transaction_time
        assert_eq!(delta.sequence, 160);           // final_update_id
        assert_eq!(delta.prev_sequence, 149);      // prev_final_update_id
        assert_eq!(delta.payload["bids"], vec![(0.0024, 10.0)]);
        assert_eq!(delta.payload["asks"], vec![(0.0026, 100.0)]);
    }

    #[test]
    fn from_impl_matches_normalize() {
        let msg: BinanceDepthUpdate = serde_json::from_str(sample_raw()).unwrap();
        let via_from = OrderBookDelta::from(msg.clone());
        let via_normalize = msg.normalize();
        assert_eq!(via_from, via_normalize);
    }

    // ── Level parsing edge cases ──────────────────────────────────────────────

    #[test]
    fn zero_qty_level_is_preserved_in_payload() {
        // qty == 0.0 means "remove this level" — the normalizer passes it through
        // as-is; removal semantics are handled by Orderbook::apply_level.
        let raw = r#"{
            "e": "depthUpdate", "E": 1, "T": 1,
            "s": "ETHUSDT", "U": 1, "u": 2, "pu": 0,
            "b": [["100.0", "0"]],
            "a": []
        }"#;
        let delta: OrderBookDelta = serde_json::from_str::<BinanceDepthUpdate>(raw)
            .unwrap()
            .into();
        assert_eq!(delta.payload["bids"], vec![(100.0, 0.0)]);
        assert!(delta.payload["asks"].is_empty());
    }

    #[test]
    fn malformed_price_level_is_skipped() {
        let raw = r#"{
            "e": "depthUpdate", "E": 1, "T": 1,
            "s": "BTCUSDT", "U": 1, "u": 2, "pu": 0,
            "b": [["not_a_number", "10"], ["50000.0", "1.5"]],
            "a": []
        }"#;
        let delta: OrderBookDelta = serde_json::from_str::<BinanceDepthUpdate>(raw)
            .unwrap()
            .into();
        // Malformed entry is dropped; valid entry survives.
        assert_eq!(delta.payload["bids"], vec![(50000.0, 1.5)]);
    }

    #[test]
    fn multiple_levels_preserve_order() {
        let raw = r#"{
            "e": "depthUpdate", "E": 1, "T": 1,
            "s": "BTCUSDT", "U": 1, "u": 3, "pu": 0,
            "b": [["100.0", "1"], ["200.0", "2"], ["150.0", "3"]],
            "a": []
        }"#;
        let delta: OrderBookDelta = serde_json::from_str::<BinanceDepthUpdate>(raw)
            .unwrap()
            .into();
        assert_eq!(
            delta.payload["bids"],
            vec![(100.0, 1.0), (200.0, 2.0), (150.0, 3.0)]
        );
    }

    // ── Roundtrip ─────────────────────────────────────────────────────────────

    #[test]
    fn wire_message_roundtrips_through_serde() {
        let msg: BinanceDepthUpdate = serde_json::from_str(sample_raw()).unwrap();
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: BinanceDepthUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, msg2);
    }
}
