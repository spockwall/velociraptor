use crate::connection::{BaseConnectionMessage, BasicConnectionMsgTrait};
use chrono::{DateTime, Utc};
use libs::protocol::ExchangeName;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenericOrder {
    pub price: f64,
    pub side: String,
    pub qty: f64,
    pub symbol: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderbookLevel {
    LevelOne,
    LevelTwo,
    LevelThree,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderInfo {
    pub price: f64,
    pub side: OrderSide,
    pub qty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderbookAction {
    Snapshot,
    Update,
    Insert,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderSide {
    Ask,
    Bid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookUpdate {
    pub action: OrderbookAction,
    pub orders: Vec<GenericOrder>,
    pub symbol: String,
    pub timestamp: DateTime<Utc>,
    pub exchange: ExchangeName,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderbookMessage {
    OrderbookUpdate(OrderbookUpdate),
    Base(BaseConnectionMessage),
}

impl BasicConnectionMsgTrait for OrderbookMessage {
    fn connected() -> Self {
        Self::Base(BaseConnectionMessage::Connected)
    }

    fn disconnected() -> Self {
        Self::Base(BaseConnectionMessage::Disconnected)
    }

    fn ping() -> Self {
        Self::Base(BaseConnectionMessage::Ping)
    }

    fn pong() -> Self {
        Self::Base(BaseConnectionMessage::Pong)
    }

    fn error(error: String) -> Self {
        Self::Base(BaseConnectionMessage::Error(error))
    }
}
