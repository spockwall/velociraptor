use crate::connection::{BasicClientMsgTrait, MsgParserTrait};
use crate::exchanges::coinbase::types::{CoinbaseLevel2Update, CoinbaseMatch, CoinbaseSnapshot};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use libs::protocol::{ExchangeName, LastTradePrice};
use libs::time::{now_ns, parse_rfc3339_to_ns};
use tracing::{error, info, warn};

pub struct CoinbaseMessageParser;

impl CoinbaseMessageParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_snapshot(&self, snapshot: CoinbaseSnapshot) -> StreamMessage {
        let recv_timestamp = now_ns();
        let symbol = snapshot.product_id;
        let mut orders = Vec::with_capacity(snapshot.bids.len() + snapshot.asks.len());

        for level in snapshot.asks {
            if let Some(order) = parse_level(&symbol, "Ask", &level, 0, recv_timestamp) {
                orders.push(order);
            }
        }
        for level in snapshot.bids {
            if let Some(order) = parse_level(&symbol, "Bid", &level, 0, recv_timestamp) {
                orders.push(order);
            }
        }

        StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            ex_timestamp: 0,
            recv_timestamp,
            exchange: ExchangeName::Coinbase,
        })
    }

    fn parse_level2_update(&self, update: CoinbaseLevel2Update) -> Vec<StreamMessage> {
        let recv_timestamp = now_ns();
        let ex_timestamp = update.time.as_deref().map(parse_rfc3339_to_ns).unwrap_or(0);
        let symbol = update.product_id;
        let mut messages = Vec::with_capacity(update.changes.len());

        // Keep each price-level change separate. A Coinbase batch can mix
        // replacements and deletions, which require different actions.
        for [maker_side, price, size] in update.changes {
            let side = match maker_side.as_str() {
                "buy" => "Bid",
                "sell" => "Ask",
                other => {
                    warn!(side = other, "Ignoring Coinbase level with unknown side");
                    continue;
                }
            };
            let Some(order) =
                parse_level(&symbol, side, &[price, size], ex_timestamp, recv_timestamp)
            else {
                continue;
            };
            let action = if order.qty == 0.0 {
                OrderbookAction::Delete
            } else {
                OrderbookAction::Update
            };
            messages.push(StreamMessage::OrderbookUpdate(OrderbookUpdate {
                action,
                orders: vec![order],
                symbol: symbol.clone(),
                ex_timestamp,
                recv_timestamp,
                exchange: ExchangeName::Coinbase,
            }));
        }

        messages
    }

    fn parse_match(&self, trade: CoinbaseMatch) -> Option<StreamMessage> {
        let price = parse_number("trade price", &trade.price)?;
        let size = parse_number("trade size", &trade.size)?;
        // Coinbase documents this field as the resting maker order's side.
        let side = match trade.side.as_str() {
            "sell" => "BUY",
            "buy" => "SELL",
            other => {
                warn!(
                    side = other,
                    "Ignoring Coinbase match with unknown maker side"
                );
                return None;
            }
        };

        Some(StreamMessage::LastTradePrice(LastTradePrice {
            exchange: ExchangeName::Coinbase,
            symbol: trade.product_id,
            full_slug: None,
            price,
            size,
            side: side.to_string(),
            fee_rate_bps: 0.0,
            market: String::new(),
            ex_timestamp: parse_rfc3339_to_ns(&trade.time),
            recv_timestamp: now_ns(),
            trade_id: Some(trade.trade_id.into()),
        }))
    }
}

impl Default for CoinbaseMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgParserTrait<StreamMessage> for CoinbaseMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(e) => {
                error!("Failed to parse Coinbase JSON: {e} - {text}");
                return Ok(vec![]);
            }
        };

        match value.get("type").and_then(|kind| kind.as_str()) {
            Some("subscriptions") => {
                info!("Coinbase subscription confirmed");
                Ok(vec![])
            }
            Some("snapshot") => match serde_json::from_value::<CoinbaseSnapshot>(value) {
                Ok(snapshot) => Ok(vec![self.parse_snapshot(snapshot)]),
                Err(e) => {
                    error!("Failed to parse Coinbase snapshot: {e} - {text}");
                    Ok(vec![])
                }
            },
            Some("l2update") => match serde_json::from_value::<CoinbaseLevel2Update>(value) {
                Ok(update) => Ok(self.parse_level2_update(update)),
                Err(e) => {
                    error!("Failed to parse Coinbase Level 2 update: {e} - {text}");
                    Ok(vec![])
                }
            },
            Some("match") => match serde_json::from_value::<CoinbaseMatch>(value) {
                Ok(trade) => Ok(self.parse_match(trade).into_iter().collect()),
                Err(e) => {
                    error!("Failed to parse Coinbase match: {e} - {text}");
                    Ok(vec![])
                }
            },
            // The first matches-channel payload is a replay of the most recent
            // historical execution. Recording it would duplicate data after
            // every reconnect.
            Some("last_match") => Ok(vec![]),
            Some("error") => {
                let message = value
                    .get("message")
                    .or_else(|| value.get("reason"))
                    .and_then(|message| message.as_str())
                    .unwrap_or("Unknown Coinbase error")
                    .to_string();
                error!("Coinbase error: {message}");
                Ok(vec![StreamMessage::error(message)])
            }
            Some(_) | None => Ok(vec![]),
        }
    }

    // Coinbase uses protocol-level WebSocket ping/pong frames.
    fn build_ping(&self) -> Option<String> {
        None
    }

    fn is_pong(&self, _text: &str) -> bool {
        false
    }
}

fn parse_level(
    symbol: &str,
    side: &str,
    level: &[String; 2],
    ex_timestamp: i64,
    recv_timestamp: i64,
) -> Option<GenericOrder> {
    Some(GenericOrder {
        price: parse_number("book price", &level[0])?,
        qty: parse_number("book size", &level[1])?,
        side: side.to_string(),
        symbol: symbol.to_string(),
        ex_timestamp,
        recv_timestamp,
    })
}

fn parse_number(field: &str, value: &str) -> Option<f64> {
    match value.parse() {
        Ok(number) => Some(number),
        Err(e) => {
            error!(field, value, "Invalid Coinbase number: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::TradeId;

    #[test]
    fn parses_level2_snapshot() {
        let raw = r#"{
          "type":"snapshot",
          "product_id":"BTC-USD",
          "bids":[["10101.10","0.45054140"]],
          "asks":[["10102.55","0.57753524"]]
        }"#;
        let messages = CoinbaseMessageParser::new().parse_message(raw).unwrap();
        assert_eq!(messages.len(), 1);

        match &messages[0] {
            StreamMessage::OrderbookUpdate(update) => {
                assert_eq!(update.action, OrderbookAction::Snapshot);
                assert_eq!(update.exchange, ExchangeName::Coinbase);
                assert_eq!(update.symbol, "BTC-USD");
                assert_eq!(update.ex_timestamp, 0);
                assert_eq!(update.orders.len(), 2);
                assert_eq!(update.orders[0].side, "Ask");
                assert_eq!(update.orders[1].side, "Bid");
            }
            other => panic!("expected order-book snapshot, got {other:?}"),
        }
    }

    #[test]
    fn splits_mixed_level2_batch_into_update_and_delete() {
        let raw = r#"{
          "type":"l2update",
          "product_id":"BTC-USD",
          "time":"2022-08-04T15:25:05.010758Z",
          "changes":[
            ["buy","22356.270000","0.00000000"],
            ["sell","22356.300000","1.00000000"]
          ]
        }"#;
        let messages = CoinbaseMessageParser::new().parse_message(raw).unwrap();
        assert_eq!(messages.len(), 2);

        let updates: Vec<_> = messages
            .iter()
            .map(|message| match message {
                StreamMessage::OrderbookUpdate(update) => update,
                other => panic!("expected order-book update, got {other:?}"),
            })
            .collect();
        assert_eq!(updates[0].action, OrderbookAction::Delete);
        assert_eq!(updates[0].orders[0].side, "Bid");
        assert_eq!(updates[1].action, OrderbookAction::Update);
        assert_eq!(updates[1].orders[0].side, "Ask");
        assert_eq!(
            updates[0].ex_timestamp,
            parse_rfc3339_to_ns("2022-08-04T15:25:05.010758Z")
        );
    }

    #[test]
    fn parses_match_with_taker_side() {
        let raw = r#"{
          "type":"match",
          "trade_id":10,
          "sequence":50,
          "maker_order_id":"ac928c66-ca53-498f-9c13-a110027a60e8",
          "taker_order_id":"132fb6ae-456b-4654-b4e0-d681ac05cea1",
          "time":"2014-11-07T08:19:27.028459Z",
          "product_id":"BTC-USD",
          "size":"5.23512",
          "price":"400.23",
          "side":"sell"
        }"#;
        let messages = CoinbaseMessageParser::new().parse_message(raw).unwrap();
        assert_eq!(messages.len(), 1);

        match &messages[0] {
            StreamMessage::LastTradePrice(trade) => {
                assert_eq!(trade.exchange, ExchangeName::Coinbase);
                assert_eq!(trade.symbol, "BTC-USD");
                assert_eq!(trade.price, 400.23);
                assert_eq!(trade.size, 5.23512);
                assert_eq!(trade.side, "BUY");
                assert_eq!(trade.trade_id, Some(TradeId::Numeric(10)));
                assert_eq!(
                    trade.ex_timestamp,
                    parse_rfc3339_to_ns("2014-11-07T08:19:27.028459Z")
                );
            }
            other => panic!("expected trade, got {other:?}"),
        }
    }

    #[test]
    fn ignores_initial_last_match_replay() {
        let raw = r#"{
          "type":"last_match",
          "trade_id":10,
          "time":"2014-11-07T08:19:27.028459Z",
          "product_id":"BTC-USD",
          "size":"5.23512",
          "price":"400.23",
          "side":"sell"
        }"#;
        let messages = CoinbaseMessageParser::new().parse_message(raw).unwrap();
        assert!(messages.is_empty());
    }
}
