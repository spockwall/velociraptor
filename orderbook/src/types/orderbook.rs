use crate::connection::{BaseClientMessage, BasicClientMsgTrait};
use libs::protocol::{ExchangeName, LastTradePrice, UserEvent};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenericOrder {
    pub price: f64,
    pub side: String,
    pub qty: f64,
    pub symbol: String,
    /// Exchange-stamped time in Unix nanoseconds (`0` when the venue sends
    /// none). Mirrors the parent [`OrderbookUpdate::ex_timestamp`].
    pub ex_timestamp: i64,
    /// Local receive time in Unix nanoseconds.
    pub recv_timestamp: i64,
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
    /// Exchange-stamped time in Unix nanoseconds when the venue provides one
    /// (OKX, Polymarket, Hyperliquid, Kalshi delta); `0` otherwise (Binance
    /// depth, Kalshi snapshot).
    pub ex_timestamp: i64,
    /// Local receive time in Unix nanoseconds.
    pub recv_timestamp: i64,
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
