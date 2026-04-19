//! User-event topic: `user.{exchange}.{kind}`.
//!
//! Kinds: `fill | order_update | balance | position`.
//!
//! # Adding a new user-event kind
//! Add a variant to `UserEvent` in `libs::protocol` and a new match arm in
//! `UserEventTopic::topic()`. The server dispatch loop needs no changes.

use super::Topic;
use libs::protocol::UserEvent;
use tracing::warn;

/// ZMQ PUB frame for a single `UserEvent`.
pub struct UserEventTopic<'a>(pub &'a UserEvent);

impl Topic for UserEventTopic<'_> {
    fn topic(&self) -> String {
        match self.0 {
            UserEvent::Fill { exchange, .. } => format!("user.{exchange}.fill"),
            UserEvent::OrderUpdate { exchange, .. } => format!("user.{exchange}.order_update"),
            UserEvent::Balance { exchange, .. } => format!("user.{exchange}.balance"),
            UserEvent::Position { exchange, .. } => format!("user.{exchange}.position"),
        }
    }

    fn encode(&self) -> Option<Vec<u8>> {
        match rmp_serde::to_vec_named(self.0) {
            Ok(b) => Some(b),
            Err(e) => {
                warn!("UserEvent encode error: {e}");
                None
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::{Side, UserEvent};

    fn fill() -> UserEvent {
        UserEvent::Fill {
            exchange: "polymarket".into(),
            client_oid: "c1".into(),
            exchange_oid: "x1".into(),
            symbol: "TOK".into(),
            side: Side::Buy,
            px: 0.5,
            qty: 10.0,
            fee: 0.01,
            ts_ns: 0,
        }
    }

    #[test]
    fn topic_format() {
        assert_eq!(UserEventTopic(&fill()).topic(), "user.polymarket.fill");
    }

    #[test]
    fn roundtrip() {
        let ev = fill();
        let t = UserEventTopic(&ev);
        let bytes = t.encode().unwrap();
        let decoded: UserEvent = super::super::decode(&bytes).unwrap();
        let UserEvent::Fill { px, qty, symbol, .. } = decoded else { panic!() };
        assert!((px - 0.5).abs() < 1e-9);
        assert_eq!(qty, 10.0);
        assert_eq!(symbol, "TOK");
    }
}
