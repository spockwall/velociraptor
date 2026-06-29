use crate::connection::MsgParserTrait;
use crate::exchanges::hyperliquid::types::{HlBookData, HlWsMessage};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use libs::protocol::ExchangeName;
use libs::time::now_ns;
use tracing::{error, info, warn};

pub struct HyperliquidMessageParser {
    exchange_name: ExchangeName,
}

impl HyperliquidMessageParser {
    pub fn new() -> Self {
        Self {
            exchange_name: ExchangeName::Hyperliquid,
        }
    }
}

impl Default for HyperliquidMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgParserTrait<StreamMessage> for HyperliquidMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        let envelope: HlWsMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                error!("Hyperliquid: failed to parse JSON envelope: {e} — {text}");
                return Ok(vec![]);
            }
        };

        match envelope.channel.as_str() {
            "pong" => return Ok(vec![]),
            "subscriptionResponse" => {
                info!("Hyperliquid: subscription confirmed");
                return Ok(vec![]);
            }
            "l2Book" => {} // handled below
            other => {
                warn!("Hyperliquid: unrecognised channel '{other}', ignoring");
                return Ok(vec![]);
            }
        }

        let data_value = match envelope.data {
            Some(v) => v,
            None => {
                error!("Hyperliquid: l2Book message missing 'data' field — {text}");
                return Ok(vec![]);
            }
        };

        let book: HlBookData = match serde_json::from_value(data_value) {
            Ok(b) => b,
            Err(e) => {
                error!("Hyperliquid: failed to deserialise HlBookData: {e}");
                return Ok(vec![]);
            }
        };

        let symbol = book.coin.to_uppercase();
        // Hyperliquid l2Book carries the server time (`book.time`, ms).
        let ex_timestamp = (book.time as i64) * 1_000_000;
        let recv_timestamp = now_ns();

        let mut orders = Vec::new();

        // levels[0] = bids, levels[1] = asks
        for bid in &book.levels[0] {
            let Ok(price) = bid.px.parse::<f64>() else {
                error!("Hyperliquid: failed to parse bid price '{}'", bid.px);
                continue;
            };
            let Ok(qty) = bid.sz.parse::<f64>() else {
                error!("Hyperliquid: failed to parse bid size '{}'", bid.sz);
                continue;
            };
            orders.push(GenericOrder {
                price,
                side: "Bid".to_string(),
                qty,
                symbol: symbol.clone(),
                ex_timestamp,
                recv_timestamp,
            });
        }

        for ask in &book.levels[1] {
            let Ok(price) = ask.px.parse::<f64>() else {
                error!("Hyperliquid: failed to parse ask price '{}'", ask.px);
                continue;
            };
            let Ok(qty) = ask.sz.parse::<f64>() else {
                error!("Hyperliquid: failed to parse ask size '{}'", ask.sz);
                continue;
            };
            orders.push(GenericOrder {
                price,
                side: "Ask".to_string(),
                qty,
                symbol: symbol.clone(),
                ex_timestamp,
                recv_timestamp,
            });
        }

        if orders.is_empty() {
            warn!("Hyperliquid: l2Book for {symbol} had no parseable levels");
            return Ok(vec![]);
        }

        Ok(vec![StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            ex_timestamp,
            recv_timestamp,
            exchange: self.exchange_name.clone(),
        })])
    }

    /// Hyperliquid expects {"method":"ping"} as an application-level keep-alive.
    /// The infrastructure wraps this in a WebSocket binary ping control frame,
    /// which most servers (including Hyperliquid) respond to with a pong.
    fn build_ping(&self) -> Option<String> {
        Some(r#"{"method":"ping"}"#.to_string())
    }

    /// Recognise the application-level pong: {"channel":"pong"}
    fn is_pong(&self, text: &str) -> bool {
        text.contains("\"pong\"")
    }

    fn is_ping(&self, _text: &str) -> bool {
        false
    }
}
