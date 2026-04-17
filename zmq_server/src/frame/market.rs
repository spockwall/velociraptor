//! Market-data payload encoding.
//!
//! Turns an `OrderbookSnapshotPayload` into a msgpack frame shaped for the
//! requested `SubscriptionType`. Topic construction lives next to the
//! dispatcher in `control::handler` since it depends on subscription routing.

use crate::protocol::BbaPayload;
use crate::trading::events::OrderbookSnapshotPayload;
use crate::types::SubscriptionType;

/// Build the msgpack payload for one subscription type from a pre-built
/// snapshot payload. Payload encoding is msgpack (see `docs/protocol.md`).
pub fn build_payload(
    snap: &OrderbookSnapshotPayload,
    sub_type: &SubscriptionType,
) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    match sub_type {
        SubscriptionType::Snapshot => rmp_serde::to_vec_named(snap),
        SubscriptionType::Bba => rmp_serde::to_vec_named(&BbaPayload::from_snapshot(snap)),
    }
}

/// Topic for a market-data frame: `{exchange}:{symbol}`.
pub fn topic_for(exchange: &str, symbol: &str) -> String {
    format!("{exchange}:{symbol}")
}
