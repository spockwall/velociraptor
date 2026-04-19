use serde::Deserialize;

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

/// Per-subscription state (what type of data this client wants).
pub struct SubscriptionState {
    pub subscription_type: SubscriptionType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Subscribe,
    Unsubscribe,
}
