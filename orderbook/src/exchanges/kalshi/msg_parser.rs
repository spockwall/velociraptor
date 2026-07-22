use crate::connection::{BaseClientMessage, MsgParserTrait};
use crate::exchanges::kalshi::types::{
    KalshiCfBenchmarksMessage, KalshiCfBenchmarksSourceValue, KalshiCfBenchmarksValue,
    KalshiCfBenchmarksValueMsg, KalshiDeltaMsg, KalshiEnvelope, KalshiSnapshotMsg, KalshiTradeMsg,
};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use libs::protocol::{ExchangeName, LastTradePrice, TradeId};
use libs::time::{now_ns, parse_rfc3339_to_ns};
use tracing::{error, info, warn};

pub struct KalshiMessageParser {
    exchange_name: ExchangeName,
}

impl KalshiMessageParser {
    pub fn new() -> Self {
        Self {
            exchange_name: ExchangeName::Kalshi,
        }
    }
}

impl Default for KalshiMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parser for Kalshi's dedicated `cfbenchmarks_value` channel.
pub struct KalshiCfBenchmarksMessageParser;

impl KalshiCfBenchmarksMessageParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_value(&self, envelope: KalshiEnvelope) -> Result<Vec<KalshiCfBenchmarksMessage>> {
        let Some(sid) = envelope.sid else {
            error!("Kalshi CF Benchmarks: value update is missing sid");
            return Ok(vec![]);
        };
        let Some(seq) = envelope.seq else {
            error!("Kalshi CF Benchmarks: value update is missing seq");
            return Ok(vec![]);
        };

        let msg: KalshiCfBenchmarksValueMsg = match serde_json::from_value(envelope.msg) {
            Ok(msg) => msg,
            Err(err) => {
                error!("Kalshi CF Benchmarks: failed to parse value payload: {err}");
                return Ok(vec![]);
            }
        };

        let source_data: KalshiCfBenchmarksSourceValue = match serde_json::from_str(&msg.data) {
            Ok(data) => data,
            Err(err) => {
                error!("Kalshi CF Benchmarks: failed to parse nested data frame: {err}");
                return Ok(vec![]);
            }
        };

        Ok(vec![KalshiCfBenchmarksMessage::Value(Box::new(
            KalshiCfBenchmarksValue {
                sid,
                seq,
                index_id: msg.index_id,
                received_at: msg.received_at,
                raw_data: msg.data,
                source_data,
                avg_60s_data: msg.avg_60s_data,
                last_60s_windowed_average_15min: msg.last_60s_windowed_average_15min,
                recv_timestamp: now_ns(),
            },
        ))])
    }
}

impl Default for KalshiCfBenchmarksMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgParserTrait<KalshiCfBenchmarksMessage> for KalshiCfBenchmarksMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<KalshiCfBenchmarksMessage>> {
        let envelope: KalshiEnvelope = match serde_json::from_str(text) {
            Ok(envelope) => envelope,
            Err(err) => {
                error!("Kalshi CF Benchmarks: failed to parse envelope: {err} — {text}");
                return Ok(vec![]);
            }
        };

        match envelope.msg_type.as_str() {
            "cfbenchmarks_value" => self.parse_value(envelope),
            "subscribed" | "subscribe_ack" | "ok" => {
                let sid = envelope
                    .sid
                    .or_else(|| envelope.msg.get("sid").and_then(serde_json::Value::as_u64));
                info!("Kalshi CF Benchmarks: subscription confirmed (sid={sid:?})");
                Ok(vec![])
            }
            "ping" | "pong" => Ok(vec![]),
            "error" => {
                error!("Kalshi CF Benchmarks: received error from server: {text}");
                Ok(vec![KalshiCfBenchmarksMessage::Base(
                    BaseClientMessage::Error(text.to_string()),
                )])
            }
            other => {
                warn!("Kalshi CF Benchmarks: unrecognised message type '{other}', ignoring");
                Ok(vec![])
            }
        }
    }

    fn build_ping(&self) -> Option<String> {
        None
    }

    fn is_ping(&self, text: &str) -> bool {
        text.contains("\"ping\"")
    }

    fn is_pong(&self, text: &str) -> bool {
        text.contains("\"pong\"")
    }
}

impl MsgParserTrait<StreamMessage> for KalshiMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        let envelope: KalshiEnvelope = match serde_json::from_str(text) {
            Ok(e) => e,
            Err(err) => {
                error!("Kalshi: failed to parse envelope: {err} — {text}");
                return Ok(vec![]);
            }
        };

        match envelope.msg_type.as_str() {
            "orderbook_snapshot" => self.parse_snapshot(envelope.msg),
            "orderbook_delta" => self.parse_delta(envelope.msg),
            "trade" => self.parse_trade(envelope.msg),
            "subscribed" | "subscribe_ack" => {
                info!("Kalshi: subscription confirmed (sid={:?})", envelope.sid);
                Ok(vec![])
            }
            "ping" => {
                // Server-initiated ping — infrastructure calls is_ping() and
                // sends our build_ping() response back. Nothing to emit here.
                Ok(vec![])
            }
            "pong" => Ok(vec![]),
            "error" => {
                error!("Kalshi: received error from server: {text}");
                Ok(vec![])
            }
            other => {
                warn!("Kalshi: unrecognised message type '{other}', ignoring");
                Ok(vec![])
            }
        }
    }

    /// Kalshi uses server-initiated pings: `{"id": N, "type": "ping"}`.
    /// We don't send client pings; return None so ConnectionBase sends a
    /// bare WebSocket ping control frame instead.
    fn build_ping(&self) -> Option<String> {
        None
    }

    /// Match Kalshi server pings so ConnectionBase can reply.
    fn is_ping(&self, text: &str) -> bool {
        text.contains("\"ping\"")
    }

    fn is_pong(&self, text: &str) -> bool {
        text.contains("\"pong\"")
    }
}

impl KalshiMessageParser {
    fn parse_trade(&self, msg: serde_json::Value) -> Result<Vec<StreamMessage>> {
        let trade: KalshiTradeMsg = match serde_json::from_value(msg) {
            Ok(trade) => trade,
            Err(err) => {
                error!("Kalshi: failed to deserialise public trade: {err}");
                return Ok(vec![]);
            }
        };

        let price = match trade.yes_price_dollars.parse::<f64>() {
            Ok(price) => price,
            Err(err) => {
                error!(
                    "Kalshi: failed to parse public trade YES price '{}': {err}",
                    trade.yes_price_dollars
                );
                return Ok(vec![]);
            }
        };
        let size = match trade.count_fp.parse::<f64>() {
            Ok(size) => size,
            Err(err) => {
                error!(
                    "Kalshi: failed to parse public trade count '{}': {err}",
                    trade.count_fp
                );
                return Ok(vec![]);
            }
        };
        let side = match trade.taker_side.to_ascii_lowercase().as_str() {
            "yes" | "buy" => "BUY",
            "no" | "sell" => "SELL",
            other => {
                warn!("Kalshi: unrecognised public trade taker side '{other}'");
                return Ok(vec![]);
            }
        };
        let ticker = trade.market_ticker;

        Ok(vec![StreamMessage::LastTradePrice(LastTradePrice {
            exchange: self.exchange_name,
            symbol: ticker.clone(),
            full_slug: None,
            // The common trade model is from the YES-contract perspective.
            price,
            size,
            side: side.to_string(),
            fee_rate_bps: 0.0,
            market: ticker,
            ex_timestamp: trade.ts_ms.saturating_mul(1_000_000),
            recv_timestamp: now_ns(),
            trade_id: Some(TradeId::Text(trade.trade_id)),
        })])
    }

    fn parse_snapshot(&self, msg: serde_json::Value) -> Result<Vec<StreamMessage>> {
        let snap: KalshiSnapshotMsg = match serde_json::from_value(msg) {
            Ok(s) => s,
            Err(e) => {
                error!("Kalshi: failed to deserialise snapshot msg: {e}");
                return Ok(vec![]);
            }
        };

        let symbol = snap.market_ticker.clone();
        // Kalshi snapshots carry no server timestamp — receive time only.
        let ex_timestamp = 0;
        let recv_timestamp = now_ns();
        let mut orders = Vec::new();

        // Kalshi sends two bid-ladders per market: one for YES buyers, one for
        // NO buyers. A traditional two-sided book is built from the YES
        // contract's perspective: NO bids at price `p` are equivalent to YES
        // asks at `1 - p` (binary-market complement).
        for level in &snap.yes_dollars_fp {
            let (price, qty) = match parse_level(level, "yes", &symbol) {
                Some(v) => v,
                None => continue,
            };
            orders.push(GenericOrder {
                price,
                qty,
                side: "Bid".to_string(),
                symbol: symbol.clone(),
                ex_timestamp,
                recv_timestamp,
            });
        }

        for level in &snap.no_dollars_fp {
            let (price, qty) = match parse_level(level, "no", &symbol) {
                Some(v) => v,
                None => continue,
            };
            orders.push(GenericOrder {
                price: 1.0 - price,
                qty,
                side: "Ask".to_string(),
                symbol: symbol.clone(),
                ex_timestamp,
                recv_timestamp,
            });
        }

        // An empty snapshot is normal: Kalshi sends one immediately on
        // subscribe, and freshly-opened 15-min windows may have zero levels
        // until the first quote arrives. Emit it anyway so the engine
        // registers the book.
        Ok(vec![StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            ex_timestamp,
            recv_timestamp,
            exchange: self.exchange_name.clone(),
        })])
    }

    fn parse_delta(&self, msg: serde_json::Value) -> Result<Vec<StreamMessage>> {
        let delta: KalshiDeltaMsg = match serde_json::from_value(msg) {
            Ok(d) => d,
            Err(e) => {
                error!("Kalshi: failed to deserialise delta msg: {e}");
                return Ok(vec![]);
            }
        };

        let symbol = delta.market_ticker.clone();

        let price: f64 = match delta.price_dollars.parse() {
            Ok(p) => p,
            Err(_) => {
                error!(
                    "Kalshi: failed to parse delta price '{}'",
                    delta.price_dollars
                );
                return Ok(vec![]);
            }
        };

        let delta_val: f64 = match delta.delta_fp.parse() {
            Ok(d) => d,
            Err(_) => {
                error!("Kalshi: failed to parse delta_fp '{}'", delta.delta_fp);
                return Ok(vec![]);
            }
        };

        // Kalshi deltas may carry a server `ts` (RFC3339); 0 when absent.
        let ex_timestamp = delta.ts.as_deref().map(parse_rfc3339_to_ns).unwrap_or(0);
        let recv_timestamp = now_ns();

        // View the book from the YES contract's perspective: YES-side deltas
        // are bids at `price`; NO-side deltas are asks at `1 - price`.
        let (side, book_price) = match delta.side.to_lowercase().as_str() {
            "yes" => ("Bid", price),
            "no" => ("Ask", 1.0 - price),
            other => {
                error!("Kalshi: unrecognised delta side '{other}'");
                return Ok(vec![]);
            }
        };

        // delta_fp == 0 → remove the level; we use size=0 + Delete action.
        // delta_fp <  0 → level shrank (Kalshi always sends the *change*,
        //                  not the new total). The orderbook engine expects
        //                  absolute sizes, so for negative deltas we emit
        //                  size=0 with Delete to signal the engine to remove
        //                  or reduce the level — the engine will reconcile.
        // delta_fp >  0 → level grew; emit as Update with the delta as qty.
        let (action, qty) = if delta_val <= 0.0 {
            (OrderbookAction::Delete, 0.0)
        } else {
            (OrderbookAction::Update, delta_val)
        };

        let order = GenericOrder {
            price: book_price,
            qty,
            side: side.to_string(),
            symbol: symbol.clone(),
            ex_timestamp,
            recv_timestamp,
        };

        Ok(vec![StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action,
            orders: vec![order],
            symbol,
            ex_timestamp,
            recv_timestamp,
            exchange: self.exchange_name.clone(),
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::MsgParserTrait;
    use crate::types::orderbook::{OrderbookAction, StreamMessage};

    fn parser() -> KalshiMessageParser {
        KalshiMessageParser::new()
    }

    fn cfbenchmarks_parser() -> KalshiCfBenchmarksMessageParser {
        KalshiCfBenchmarksMessageParser::new()
    }

    #[test]
    fn parses_cfbenchmarks_value_with_both_averages() {
        let raw = r#"{
            "type": "cfbenchmarks_value",
            "sid": 1,
            "seq": 42,
            "msg": {
                "index_id": "BRTI",
                "received_at": 1710000000123,
                "data": "{\"type\":\"value\",\"id\":\"BRTI\",\"time\":1710000000123,\"value\":\"68000.12\"}",
                "avg_60s_data": {
                    "value": "68000.12000000",
                    "window_size": 3,
                    "window_start_ts_ms": 1709999940123,
                    "window_end_ts_exclusive": 1710000000123
                },
                "last_60s_windowed_average_15min": {
                    "value": "68000.23000000",
                    "window_size": 14,
                    "window_start_ts_ms": 1709999980000,
                    "window_end_ts_exclusive": 1710000000123
                }
            }
        }"#;

        let messages = cfbenchmarks_parser().parse_message(raw).unwrap();
        assert_eq!(messages.len(), 1);

        let KalshiCfBenchmarksMessage::Value(value) = &messages[0] else {
            panic!("expected CF Benchmarks value, got {:?}", messages[0]);
        };
        assert_eq!(value.sid, 1);
        assert_eq!(value.seq, 42);
        assert_eq!(value.index_id, "BRTI");
        assert_eq!(value.received_at, 1710000000123);
        assert_eq!(value.source_data.value_type, "value");
        assert_eq!(value.source_data.id, "BRTI");
        assert_eq!(value.source_data.time, 1710000000123);
        assert_eq!(value.source_data.value, "68000.12");
        assert!(value.raw_data.contains("68000.12"));
        assert_eq!(value.avg_60s_data.value, "68000.12000000");
        assert_eq!(value.avg_60s_data.window_size, 3);
        assert_eq!(
            value
                .last_60s_windowed_average_15min
                .as_ref()
                .unwrap()
                .value,
            "68000.23000000"
        );
        assert!(value.recv_timestamp > 0);
    }

    #[test]
    fn parses_cfbenchmarks_value_without_quarter_hour_average() {
        let raw = r#"{
            "type": "cfbenchmarks_value",
            "sid": 1,
            "seq": 43,
            "msg": {
                "index_id": "ETHUSD_RTI",
                "received_at": 1710000001123,
                "data": "{\"type\":\"value\",\"id\":\"ETHUSD_RTI\",\"time\":1710000001123,\"value\":\"3500.01\"}",
                "avg_60s_data": {
                    "value": "3500.01000000",
                    "window_size": 1,
                    "window_start_ts_ms": 1709999941123,
                    "window_end_ts_exclusive": 1710000001123
                }
            }
        }"#;

        let messages = cfbenchmarks_parser().parse_message(raw).unwrap();
        let KalshiCfBenchmarksMessage::Value(value) = &messages[0] else {
            panic!("expected CF Benchmarks value, got {:?}", messages[0]);
        };
        assert_eq!(value.index_id, "ETHUSD_RTI");
        assert_eq!(value.source_data.value, "3500.01");
        assert!(value.last_60s_windowed_average_15min.is_none());
    }

    #[test]
    fn drops_cfbenchmarks_value_with_malformed_nested_data() {
        let raw = r#"{
            "type": "cfbenchmarks_value",
            "sid": 1,
            "seq": 44,
            "msg": {
                "index_id": "BRTI",
                "received_at": 1710000002123,
                "data": "not-json",
                "avg_60s_data": {
                    "value": "68000.12",
                    "window_size": 1,
                    "window_start_ts_ms": 1709999942123,
                    "window_end_ts_exclusive": 1710000002123
                }
            }
        }"#;

        assert!(cfbenchmarks_parser().parse_message(raw).unwrap().is_empty());
    }

    #[test]
    fn exposes_cfbenchmarks_server_errors() {
        let raw = r#"{"id":9,"type":"error","msg":{"code":24,"msg":"Index IDs required"}}"#;
        let messages = cfbenchmarks_parser().parse_message(raw).unwrap();

        let KalshiCfBenchmarksMessage::Base(BaseClientMessage::Error(message)) = &messages[0]
        else {
            panic!("expected base error, got {:?}", messages[0]);
        };
        assert!(message.contains("Index IDs required"));
    }

    #[test]
    fn parses_public_trade_with_uuid_and_yes_perspective() {
        let raw = r#"{
            "type": "trade",
            "sid": 11,
            "msg": {
                "trade_id": "d91bc706-ee49-470d-82d8-11418bda6fed",
                "market_ticker": "KXBTC15M-26JUL221200-00",
                "yes_price_dollars": "0.360",
                "no_price_dollars": "0.640",
                "count_fp": "136.00",
                "taker_side": "no",
                "taker_outcome_side": "no",
                "taker_book_side": "ask",
                "ts": 1669149841,
                "ts_ms": 1669149841000
            }
        }"#;

        let messages = parser().parse_message(raw).unwrap();
        assert_eq!(messages.len(), 1);
        let StreamMessage::LastTradePrice(trade) = &messages[0] else {
            panic!("expected public trade, got {:?}", messages[0]);
        };

        assert_eq!(trade.symbol, "KXBTC15M-26JUL221200-00");
        assert!((trade.price - 0.36).abs() < 1e-9);
        assert!((trade.size - 136.0).abs() < 1e-9);
        assert_eq!(trade.side, "SELL");
        assert_eq!(trade.ex_timestamp, 1_669_149_841_000_000_000);
        assert_eq!(
            trade.trade_id,
            Some(TradeId::Text(
                "d91bc706-ee49-470d-82d8-11418bda6fed".to_string()
            ))
        );
        assert!(trade.recv_timestamp > 0);
    }

    #[test]
    fn parses_legacy_public_trade_without_new_taker_fields() {
        let raw = r#"{
            "type": "trade",
            "sid": 11,
            "msg": {
                "trade_id": "d91bc706-ee49-470d-82d8-11418bda6fed",
                "market_ticker": "HIGHNY-22DEC23-B53.5",
                "yes_price_dollars": "0.360",
                "no_price_dollars": "0.640",
                "count_fp": "136.00",
                "taker_side": "no",
                "ts": 1669149841,
                "ts_ms": 1669149841000
            }
        }"#;

        let messages = parser().parse_message(raw).unwrap();
        let StreamMessage::LastTradePrice(trade) = &messages[0] else {
            panic!("expected public trade, got {:?}", messages[0]);
        };
        assert_eq!(trade.symbol, "HIGHNY-22DEC23-B53.5");
        assert!((trade.price - 0.36).abs() < 1e-9);
        assert!((trade.size - 136.0).abs() < 1e-9);
        assert_eq!(trade.side, "SELL");
        assert_eq!(trade.ex_timestamp, 1_669_149_841_000_000_000);
        assert_eq!(
            trade.trade_id,
            Some(TradeId::Text(
                "d91bc706-ee49-470d-82d8-11418bda6fed".to_string()
            ))
        );
    }

    /// Real wire payload captured from Kalshi's WebSocket for an
    /// `orderbook_snapshot` frame — fields are string decimals, not cents.
    #[test]
    fn parses_snapshot() {
        let raw = r#"{
            "type": "orderbook_snapshot",
            "sid": 2,
            "seq": 2,
            "msg": {
                "market_ticker": "FED-23DEC-T3.00",
                "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "yes_dollars_fp": [
                    ["0.0800", "300.00"],
                    ["0.2200", "333.00"]
                ],
                "no_dollars_fp": [
                    ["0.5400", "20.00"],
                    ["0.5600", "146.00"]
                ]
            }
        }"#;

        let msgs = parser().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);

        if let StreamMessage::OrderbookUpdate(u) = &msgs[0] {
            assert_eq!(u.action, OrderbookAction::Snapshot);
            assert_eq!(u.symbol, "FED-23DEC-T3.00");
            assert_eq!(u.orders.len(), 4);

            let bids: Vec<_> = u.orders.iter().filter(|o| o.side == "Bid").collect();
            let asks: Vec<_> = u.orders.iter().filter(|o| o.side == "Ask").collect();
            assert_eq!(bids.len(), 2);
            assert_eq!(asks.len(), 2);

            // YES bid at 0.08 stays as-is; NO bid at 0.54 becomes YES ask at 1 - 0.54 = 0.46.
            assert!((bids[0].price - 0.08).abs() < 1e-9);
            assert!((bids[0].qty - 300.0).abs() < 1e-9);
            assert!((asks[0].price - 0.46).abs() < 1e-9);
            assert!((asks[0].qty - 20.0).abs() < 1e-9);
        } else {
            panic!("Expected OrderbookUpdate");
        }
    }

    /// Real wire payload for an `orderbook_delta` frame: string decimal
    /// `price_dollars` + signed string `delta_fp`.
    #[test]
    fn parses_positive_delta() {
        let raw = r#"{
            "type": "orderbook_delta",
            "sid": 2,
            "seq": 3,
            "msg": {
                "market_ticker": "FED-23DEC-T3.00",
                "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "price_dollars": "0.960",
                "delta_fp": "54.00",
                "side": "yes",
                "ts": "2022-11-22T20:44:01Z"
            }
        }"#;

        let msgs = parser().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);

        if let StreamMessage::OrderbookUpdate(u) = &msgs[0] {
            assert_eq!(u.action, OrderbookAction::Update);
            assert_eq!(u.orders[0].side, "Bid");
            assert!((u.orders[0].price - 0.960).abs() < 1e-9);
            assert!((u.orders[0].qty - 54.0).abs() < 1e-9);
        } else {
            panic!("Expected OrderbookUpdate");
        }
    }

    /// Exact payload the user pasted: negative delta on the YES side.
    #[test]
    fn parses_negative_delta_as_delete() {
        let raw = r#"{
            "type": "orderbook_delta",
            "sid": 2,
            "seq": 3,
            "msg": {
                "market_ticker": "FED-23DEC-T3.00",
                "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "price_dollars": "0.960",
                "delta_fp": "-54.00",
                "side": "yes",
                "ts": "2022-11-22T20:44:01Z"
            }
        }"#;

        let msgs = parser().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);

        if let StreamMessage::OrderbookUpdate(u) = &msgs[0] {
            assert_eq!(u.action, OrderbookAction::Delete);
            // side: "yes" → bid
            assert_eq!(u.orders[0].side, "Bid");
            assert_eq!(u.orders[0].qty, 0.0);
            assert!((u.orders[0].price - 0.960).abs() < 1e-9);
        } else {
            panic!("Expected OrderbookUpdate");
        }
    }

    /// NO-side delta: price in the book is the complement (`1 - p`) and
    /// the side is Ask.
    #[test]
    fn parses_no_side_delta_as_complemented_ask() {
        let raw = r#"{
            "type": "orderbook_delta",
            "sid": 2,
            "seq": 4,
            "msg": {
                "market_ticker": "FED-23DEC-T3.00",
                "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "price_dollars": "0.56",
                "delta_fp": "10.00",
                "side": "no",
                "ts": "2022-11-22T20:44:01Z"
            }
        }"#;

        let msgs = parser().parse_message(raw).unwrap();
        if let StreamMessage::OrderbookUpdate(u) = &msgs[0] {
            assert_eq!(u.action, OrderbookAction::Update);
            assert_eq!(u.orders[0].side, "Ask");
            assert!((u.orders[0].price - 0.44).abs() < 1e-9); // 1 - 0.56
            assert!((u.orders[0].qty - 10.0).abs() < 1e-9);
        } else {
            panic!("Expected OrderbookUpdate");
        }
    }

    #[test]
    fn ignores_subscribed_and_ping() {
        let subscribed = r#"{"type":"subscribed","sid":1,"seq":1,"msg":{}}"#;
        assert!(parser().parse_message(subscribed).unwrap().is_empty());

        let ping = r#"{"type":"ping","id":42}"#;
        assert!(parser().parse_message(ping).unwrap().is_empty());
    }

    #[test]
    fn is_ping_detection() {
        let p = parser();
        assert!(p.is_ping(r#"{"type":"ping","id":1}"#));
        assert!(!p.is_ping(r#"{"type":"orderbook_snapshot","sid":1}"#));
    }

    #[test]
    fn subscription_builder() {
        use crate::exchanges::kalshi::KalshiSubMsgBuilder;
        let msg = KalshiSubMsgBuilder::new()
            .with_orderbook_channel()
            .with_ticker("FED-23DEC-T3.00")
            .with_ticker("PRES-2028")
            .build();
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["cmd"], "subscribe");
        let channels = v["params"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0], "orderbook_delta");
        let tickers = v["params"]["market_tickers"].as_array().unwrap();
        assert_eq!(tickers.len(), 2);
        assert_eq!(tickers[0], "FED-23DEC-T3.00");
        assert_eq!(tickers[1], "PRES-2028");
    }
}

/// Parse a `[price_str, size_str]` level pair, logging errors on failure.
fn parse_level(level: &[String; 2], side: &str, symbol: &str) -> Option<(f64, f64)> {
    let price: f64 = match level[0].parse() {
        Ok(p) => p,
        Err(_) => {
            error!(
                "Kalshi: failed to parse {side} price '{}' for {symbol}",
                level[0]
            );
            return None;
        }
    };
    let qty: f64 = match level[1].parse() {
        Ok(q) => q,
        Err(_) => {
            error!(
                "Kalshi: failed to parse {side} size '{}' for {symbol}",
                level[1]
            );
            return None;
        }
    };
    Some((price, qty))
}
