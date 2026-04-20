use super::orders::{OrderStatus, Side};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Slim best-bid-ask payload stored in Redis and served by the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BbaPayload {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserEvent {
    Balance {
        exchange: String,
        asset: String,
        free: f64,
        locked: f64,
        ts_ns: i64,
    },
    Position {
        exchange: String,
        symbol: String,
        size: f64,
        avg_px: f64,
        ts_ns: i64,
    },
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
        client_oid: String,
        exchange_oid: String,
        symbol: String,
        side: Side,
        px: f64,
        qty: f64,
        fee: f64,
        ts_ns: i64,
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
            client_oid: "c1".into(),
            exchange_oid: "x1".into(),
            symbol: "TRUMP-2028".into(),
            side: Side::Buy,
            px: 0.42,
            qty: 10.0,
            fee: 0.01,
            ts_ns: 1_700_000_000_000_000_000,
        };
        let bytes = rmp_serde::to_vec_named(&ev).expect("encode");
        let decoded: UserEvent = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(ev, decoded);
    }
}
