use super::orders::{OrderStatus, Side};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Slim best-bid-ask payload stored in Redis and served by the backend.
///
/// `symbol` is the venue asset id; `full_slug` carries the window
/// identity for rolling Polymarket / Kalshi topics (mirrors
/// [`crate::protocol::OrderbookSnapshot`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BbaPayload {
    pub exchange: String,
    pub symbol: String,
    #[serde(default)]
    pub full_slug: Option<String>,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    /// Exchange event time in ns since UNIX epoch (see
    /// [`crate::protocol::OrderbookSnapshot::t_exch_ns`]). `0` if absent.
    #[serde(default)]
    pub t_exch_ns: u64,
    /// Wall-clock ns when orderbook_server read this off the WS (see
    /// [`crate::protocol::OrderbookSnapshot::t_recv_ns`]).
    #[serde(default)]
    pub t_recv_ns: u64,
    pub best_bid: Option<(f64, f64)>,
    pub best_ask: Option<(f64, f64)>,
    pub spread: Option<f64>,
}

/// Which capped Redis list / spillover stream an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Fills,
    Orders,
    Log,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Fills => "fills",
            EventKind::Orders => "orders",
            EventKind::Log => "log",
        }
    }

    pub fn redis_key(&self) -> String {
        format!("events:{}", self.as_str())
    }
}

/// Private account / trade events emitted on the user WS channels.
///
/// Exchange-agnostic by design. Polymarket's WS user channel only pushes
/// order-lifecycle and fill events; balance/position would need a REST
/// poller, which we don't run today, so the corresponding variants are
/// omitted to keep the enum honest about what actually flows.
///
/// The `Fill` variant carries an opaque `maker_orders` blob (JSON) so
/// per-exchange extension data can ride along without leaking
/// exchange-specific types into `libs::protocol`. Polymarket populates it
/// with the trade's `maker_orders` list; other exchanges leave it `None`.
/// Consumers that don't care about exchange-specific maker info ignore
/// the column; researchers can `json_normalize` it on the reader side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserEvent {
    OrderUpdate {
        exchange: String,
        client_oid: String,
        exchange_oid: String,
        symbol: String,
        side: Side,
        px: f64,
        qty: f64,
        filled: f64,
        status: OrderStatus,
        ts_ns: i64,
    },
    Fill {
        exchange: String,
        /// Taker order id, when the fill identifies a distinct taker
        /// (Polymarket trade events). `None` for venues that don't.
        taker_oid: Option<String>,
        /// Client-supplied order id, when available. `None` for fills
        /// without a known client_oid (e.g. trades on the taker side
        /// where we only know the taker_oid).
        client_oid: Option<String>,
        /// Exchange-side order id when the venue identifies a single
        /// order on the fill. `None` for venues whose fill event does
        /// not carry an order id (Polymarket trade events report a
        /// trade UUID, not an order id — use `taker_oid` / `maker_orders`
        /// to match the fill to placed orders).
        exchange_oid: Option<String>,
        /// Venue-assigned trade id when available. Polymarket emits a
        /// UUID per matched trade; other venues may leave this `None`.
        /// Distinct from `exchange_oid` (which identifies an order, not
        /// an execution).
        #[serde(default)]
        trade_id: Option<String>,
        symbol: String,
        side: Side,
        px: f64,
        qty: f64,
        fee: f64,
        ts_ns: i64,
        /// Exchange-specific trade-lifecycle status. For Polymarket this is
        /// the on-chain settlement progression: `MATCHED` → `MINED` →
        /// `CONFIRMED`, or `RETRYING` / `FAILED`. Stored as the raw
        /// upper-case string the venue emits so new venue states pass
        /// through without a code change. `None` for venues that don't
        /// publish per-trade status.
        #[serde(default)]
        trade_status: Option<String>,
        /// Exchange-specific extension data, JSON-encoded. Polymarket
        /// populates this with the trade's `maker_orders` list (a
        /// `Vec<PolyMakerOrder>`). Other exchanges set `None`.
        ///
        /// Kept opaque (`serde_json::Value`) so `libs::protocol` stays
        /// exchange-agnostic.
        maker_orders: Option<serde_json::Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_redis_key() {
        assert_eq!(EventKind::Fills.redis_key(), "events:fills");
        assert_eq!(EventKind::Orders.redis_key(), "events:orders");
        assert_eq!(EventKind::Log.redis_key(), "events:log");
    }

    #[test]
    fn user_event_roundtrip() {
        let ev = UserEvent::Fill {
            exchange: "polymarket".into(),
            taker_oid: Some("c1".into()),
            client_oid: None,
            exchange_oid: None,
            trade_id: Some("trade-abc".into()),
            symbol: "TRUMP-2028".into(),
            side: Side::Buy,
            px: 0.42,
            qty: 10.0,
            fee: 0.01,
            ts_ns: 1_700_000_000_000_000_000,
            trade_status: Some("MATCHED".into()),
            maker_orders: Some(serde_json::json!([
                {"order_id": "m1", "matched_amount": "10.0", "price": "0.42"}
            ])),
        };
        let bytes = rmp_serde::to_vec_named(&ev).expect("encode");
        let decoded: UserEvent = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(ev, decoded);
    }
}
