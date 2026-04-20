use crate::connection::{BaseClientMessage, BasicClientMsgTrait};
use chrono::{DateTime, Utc};
use libs::protocol::{ExchangeName, LastTradePrice, UserEvent};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub enum StreamMessage {
    OrderbookUpdate(OrderbookUpdate),
    UserEvent(UserEvent),
    LastTradePrice(LastTradePrice),
    Base(BaseClientMessage),
}

impl BasicClientMsgTrait for StreamMessage {
    fn connected() -> Self {
        Self::Base(BaseClientMessage::Connected)
    }

    fn disconnected() -> Self {
        Self::Base(BaseClientMessage::Disconnected)
    }

    fn ping() -> Self {
        Self::Base(BaseClientMessage::Ping)
    }

    fn pong() -> Self {
        Self::Base(BaseClientMessage::Pong)
    }

    fn error(error: String) -> Self {
        Self::Base(BaseClientMessage::Error(error))
    }
}
