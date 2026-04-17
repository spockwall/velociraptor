//! User-event PUB payload encoding.
//!
//! Produces `(topic, msgpack_payload)` frames for a `UserEvent`. Topic format
//! matches the protocol spec: `user.{exchange}.{kind}` where `kind` is one of
//! `fill | order_update | balance | position`.

use libs::protocol::UserEvent;
use tracing::warn;

pub fn topic_for(event: &UserEvent) -> String {
    match event {
        UserEvent::Fill { exchange, .. } => format!("user.{exchange}.fill"),
        UserEvent::OrderUpdate { exchange, .. } => format!("user.{exchange}.order_update"),
        UserEvent::Balance { exchange, .. } => format!("user.{exchange}.balance"),
        UserEvent::Position { exchange, .. } => format!("user.{exchange}.position"),
    }
}

/// Encode a user event as `(topic, msgpack_bytes)`.
pub fn encode(event: &UserEvent) -> Option<(String, Vec<u8>)> {
    let topic = topic_for(event);
    match rmp_serde::to_vec_named(event) {
        Ok(bytes) => Some((topic, bytes)),
        Err(e) => {
            warn!("UserPublisher: failed to msgpack-encode event: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::{Side, UserEvent};

    #[test]
    fn topic_format() {
        let fill = UserEvent::Fill {
            exchange: "polymarket".into(),
            client_oid: "c1".into(),
            exchange_oid: "x1".into(),
            symbol: "TOK".into(),
            side: Side::Buy,
            px: 0.5,
            qty: 10.0,
            fee: 0.01,
            ts_ns: 0,
        };
        assert_eq!(topic_for(&fill), "user.polymarket.fill");
    }
}
