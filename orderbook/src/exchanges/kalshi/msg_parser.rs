use crate::connection::MsgParserTrait;
use crate::exchanges::kalshi::types::{KalshiDeltaMsg, KalshiEnvelope, KalshiSnapshotMsg};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use chrono::Utc;
use libs::protocol::ExchangeName;
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
    fn parse_snapshot(&self, msg: serde_json::Value) -> Result<Vec<StreamMessage>> {
        let snap: KalshiSnapshotMsg = match serde_json::from_value(msg) {
            Ok(s) => s,
            Err(e) => {
                error!("Kalshi: failed to deserialise snapshot msg: {e}");
                return Ok(vec![]);
            }
        };

        let symbol = snap.market_ticker.clone();
        let timestamp = Utc::now();
        let ts_str = timestamp.to_rfc3339();
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
                timestamp: ts_str.clone(),
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
                timestamp: ts_str.clone(),
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
            timestamp,
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

        // Determine timestamp: prefer server ts, fall back to now.
        let timestamp = delta
            .ts
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let ts_str = timestamp.to_rfc3339();

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
            timestamp: ts_str,
        };

        Ok(vec![StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action,
            orders: vec![order],
            symbol,
            timestamp,
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
