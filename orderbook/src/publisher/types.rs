use serde::Deserialize;
use std::time::{Duration, Instant};

/// The data type a client subscribes to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionType {
    Snapshot,
    Bba,
}

/// Uniquely identifies one client's subscription to one symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionKey {
    pub client_id: Vec<u8>,
    pub exchange: String,
    pub symbol: String,
}

/// Per-subscription throttle state.
pub struct SubscriptionState {
    pub subscription_type: SubscriptionType,
    pub interval: Duration,
    pub last_sent: Instant,
}

// Client actions
#[derive(Debug, Deserialize)]
pub enum Action {
    Subscribe,
    Unsubscribe,
}
