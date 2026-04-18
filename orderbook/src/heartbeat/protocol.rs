use crate::connection::{BasicClientMsgTrait, MsgParserTrait};
use libs::protocol::ExchangeName;
use tokio_tungstenite::tungstenite::Message;

pub struct HearthbeatProtocol<'a, M: BasicClientMsgTrait> {
    parser: &'a dyn MsgParserTrait<M>,
    exchange_name: &'a ExchangeName,
}

/// Protocol handler for ping/pong messages
impl<'a, M: BasicClientMsgTrait> HearthbeatProtocol<'a, M> {
    pub fn new(parser: &'a dyn MsgParserTrait<M>, exchange_name: &'a ExchangeName) -> Self {
        Self {
            parser,
            exchange_name,
        }
    }

    pub fn get_exchange_name(&self) -> &ExchangeName {
        self.exchange_name
    }

    /// Build ping message for this exchange
    pub fn build_ping(&self) -> Message {
        match self.parser.build_ping() {
            Some(text) => Message::Ping(text.into()),
            None => Message::Ping(vec![].into()),
        }
    }

    /// Check if message is a pong
    pub fn is_pong(&self, msg: &Message) -> bool {
        match msg {
            Message::Text(text) => self.parser.is_pong(text),
            Message::Pong(_) => true,
            _ => false,
        }
    }

    /// Check if message is a ping requiring response
    pub fn is_ping(&self, msg: &Message) -> bool {
        match msg {
            Message::Text(text) => self.parser.is_ping(text),
            Message::Ping(_) => true,
            _ => false,
        }
    }
}
