use crate::connection::MsgParserTrait;
use crate::exchanges::binance::types::{
    BinanceDepthData, BinanceSubscribeResponse, BinanceTradeEvent,
};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use libs::protocol::{ExchangeName, LastTradePrice};
use tracing::{error, info};

pub struct BinanceMessageParser {
    exchange_name: ExchangeName,
}

impl BinanceMessageParser {
    pub fn new() -> Self {
        Self {
            exchange_name: ExchangeName::Binance,
        }
    }

    pub fn with_exchange(exchange_name: ExchangeName) -> Self {
        Self { exchange_name }
    }

    fn parse_trade(&self, ev: BinanceTradeEvent) -> Option<StreamMessage> {
        let price: f64 = ev.price.parse().ok()?;
        let qty: f64 = ev.qty.parse().ok()?;
        let ts = Utc
            .timestamp_millis_opt(ev.trade_time)
            .single()
            .unwrap_or_else(Utc::now);
        // On Binance, `m = true` means the buyer was the maker, so the taker
        // (the side that crossed the book) was the seller.
        let side = if ev.buyer_is_maker { "SELL" } else { "BUY" }.to_string();
        Some(StreamMessage::LastTradePrice(LastTradePrice {
            exchange: self.exchange_name.clone(),
            symbol: ev.symbol.to_lowercase(),
            price,
            size: qty,
            side,
            fee_rate_bps: 0.0,
            market: String::new(),
            timestamp: ts,
            trade_id: Some(ev.trade_id),
        }))
    }

    fn dispatch(&self, value: serde_json::Value, text: &str) -> Result<Vec<StreamMessage>> {
        // Trade stream is the only event with `e == "trade"`. Everything else
        // (spot `@depth20@100ms` has no `e`; futures `@depth20@100ms` has
        // `e == "depthUpdate"`) is routed to the depth parser.
        if value.get("e").and_then(|v| v.as_str()) == Some("trade") {
            return match serde_json::from_value::<BinanceTradeEvent>(value) {
                Ok(ev) => Ok(self.parse_trade(ev).into_iter().collect()),
                Err(e) => {
                    error!("Failed to parse Binance trade: {e} - {text}");
                    Ok(vec![])
                }
            };
        }
        match serde_json::from_value::<BinanceDepthData>(value) {
            Ok(msg) => Ok(self.parse_depth(msg).into_iter().collect()),
            Err(e) => {
                error!("Failed to parse Binance depth: {e} - {text}");
                Ok(vec![])
            }
        }
    }

    fn parse_depth(&self, msg: BinanceDepthData) -> Option<StreamMessage> {
        let symbol = msg.symbol.to_lowercase();
        let timestamp = Utc::now();
        let ts_str = timestamp.to_rfc3339();

        let mut orders = Vec::with_capacity(msg.bids.len() + msg.asks.len());

        for ask in &msg.asks {
            let price: f64 = ask[0].parse().unwrap_or(0.0);
            let qty: f64 = ask[1].parse().unwrap_or(0.0);
            orders.push(GenericOrder {
                price,
                side: "Ask".to_string(),
                qty,
                symbol: symbol.clone(),
                timestamp: ts_str.clone(),
            });
        }

        for bid in &msg.bids {
            let price: f64 = bid[0].parse().unwrap_or(0.0);
            let qty: f64 = bid[1].parse().unwrap_or(0.0);
            orders.push(GenericOrder {
                price,
                side: "Bid".to_string(),
                qty,
                symbol: symbol.clone(),
                timestamp: ts_str.clone(),
            });
        }

        if orders.is_empty() {
            return None;
        }

        Some(StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            timestamp,
            exchange: self.exchange_name.clone(),
        }))
    }
}

impl Default for BinanceMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgParserTrait<StreamMessage> for BinanceMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        // Subscription confirmation: {"result":null,"id":1}
        if let Ok(resp) = serde_json::from_str::<BinanceSubscribeResponse>(text) {
            if resp.id.is_some() && resp.result.is_none() {
                info!("Binance subscription confirmed");
                return Ok(vec![]);
            }
        }

        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse Binance JSON: {e} - {text}");
                return Ok(vec![]);
            }
        };

        // Combined-stream wrapper (spot uses `/stream` endpoint):
        // {"stream":"btcusdt@depth20@100ms","data":{...}}
        // The inner `data` for `@depth20` omits the symbol — recover it from the
        // stream name and inject before dispatching.
        if let (Some(stream), Some(data)) = (value.get("stream"), value.get("data")) {
            if let (Some(stream_s), Some(_)) = (stream.as_str(), data.as_object()) {
                let symbol = stream_s.split('@').next().unwrap_or("").to_string();
                let mut inner = data.clone();
                if symbol.len() > 0 {
                    if let Some(obj) = inner.as_object_mut() {
                        obj.entry("s".to_string())
                            .or_insert_with(|| serde_json::Value::String(symbol.to_uppercase()));
                    }
                }
                return self.dispatch(inner, text);
            }
        }

        self.dispatch(value, text)
    }

    // Binance uses protocol-level WebSocket ping frames — no application-level ping needed
    fn build_ping(&self) -> Option<String> {
        None
    }

    fn is_pong(&self, _text: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trade_as_last_trade_price() {
        let parser = BinanceMessageParser::with_exchange(ExchangeName::BinanceSpot);
        let raw = r#"{"e":"trade","E":1700000000123,"s":"BTCUSDT","t":12345,"p":"50000.10","q":"0.5","T":1700000000100,"m":false,"M":true}"#;
        let msgs = parser.parse_message(raw).expect("parse ok");
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::LastTradePrice(t) => {
                assert_eq!(t.exchange, ExchangeName::BinanceSpot);
                assert_eq!(t.symbol, "btcusdt");
                assert_eq!(t.price, 50000.10);
                assert_eq!(t.size, 0.5);
                assert_eq!(t.side, "BUY");
                assert_eq!(t.trade_id, Some(12345));
            }
            other => panic!("expected LastTradePrice, got {other:?}"),
        }
    }

    #[test]
    fn parses_depth_snapshot() {
        let parser = BinanceMessageParser::with_exchange(ExchangeName::BinanceSpot);
        let raw = r#"{"s":"BTCUSDT","b":[["49999.0","1.5"]],"a":[["50001.0","2.0"]]}"#;
        let msgs = parser.parse_message(raw).expect("parse ok");
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::OrderbookUpdate(u) => {
                assert_eq!(u.symbol, "btcusdt");
                assert_eq!(u.orders.len(), 2);
            }
            other => panic!("expected OrderbookUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_spot_combined_depth_envelope() {
        let parser = BinanceMessageParser::with_exchange(ExchangeName::BinanceSpot);
        let raw = r#"{"stream":"ethusdt@depth20@100ms","data":{"lastUpdateId":1,"bids":[["2319.26","109.02"]],"asks":[["2319.27","5.87"]]}}"#;
        let msgs = parser.parse_message(raw).expect("parse ok");
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::OrderbookUpdate(u) => {
                assert_eq!(u.symbol, "ethusdt");
                assert_eq!(u.orders.len(), 2);
            }
            other => panic!("expected OrderbookUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_spot_combined_trade_envelope() {
        let parser = BinanceMessageParser::with_exchange(ExchangeName::BinanceSpot);
        let raw = r#"{"stream":"btcusdt@trade","data":{"e":"trade","E":1700000000123,"s":"BTCUSDT","t":12345,"p":"50000.10","q":"0.5","T":1700000000100,"m":true,"M":true}}"#;
        let msgs = parser.parse_message(raw).expect("parse ok");
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::LastTradePrice(t) => {
                assert_eq!(t.symbol, "btcusdt");
                assert_eq!(t.side, "SELL");
            }
            other => panic!("expected LastTradePrice, got {other:?}"),
        }
    }

    #[test]
    fn ignores_subscription_ack() {
        let parser = BinanceMessageParser::new();
        let msgs = parser
            .parse_message(r#"{"result":null,"id":1}"#)
            .expect("parse ok");
        assert!(msgs.is_empty());
    }
}
