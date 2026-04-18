use crate::connection::MsgParserTrait;
use crate::exchanges::polymarket::types::{
    PolyOrderEvent, PolyTradeEvent, PolymarketBookEvent, PolymarketPriceChangeEvent,
};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookUpdate, StreamMessage};
use anyhow::Result;
use libs::protocol::{ExchangeName, OrderStatus, Side, UserEvent};
use libs::time::now_ns;
use libs::time::parse_timestamp_ms;
use std::collections::HashMap;
use tracing::{error, info, warn};

/// Parses all Polymarket WebSocket messages — both public market-channel events
/// and private user-channel events — into [`StreamMessage`].
///
/// ## Market channel events (`type: "market"` subscription)
/// - `"book"` → `StreamMessage::OrderbookUpdate` with `action: Snapshot`
/// - `"price_change"` → `StreamMessage::OrderbookUpdate` with `action: Update|Delete`
///
/// ## User channel events (`type: "user"` subscription)
/// - `"order"` → `StreamMessage::UserEvent(UserEvent::OrderUpdate)`
/// - `"trade"` → `StreamMessage::UserEvent(UserEvent::Fill)`
pub struct PolymarketMessageParser {
    exchange_name: ExchangeName,
}

impl PolymarketMessageParser {
    pub fn new() -> Self {
        Self {
            exchange_name: ExchangeName::Polymarket,
        }
    }
}

impl Default for PolymarketMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgParserTrait<StreamMessage> for PolymarketMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                if text.trim().eq_ignore_ascii_case("pong") {
                    return Ok(vec![]);
                }
                error!("Polymarket: failed to parse JSON: {e} — {text}");
                return Ok(vec![]);
            }
        };

        // All server messages are JSON arrays: [] (ack) or [{...event...}, ...]
        let events: Vec<serde_json::Value> = if value.is_array() {
            value.as_array().cloned().unwrap_or_default()
        } else {
            vec![value]
        };

        let mut messages = Vec::new();
        for event in &events {
            let Some(et) = event.get("event_type").and_then(|v| v.as_str()) else {
                // Empty array [] = subscription ack; single object without event_type = ignored.
                if !event.is_null() && !events.is_empty() {
                    info!("Polymarket: message without event_type (ack?): {event}");
                }
                continue;
            };

            match et {
                "book" => messages.extend(self.parse_book(event)?),
                "price_change" => messages.extend(self.parse_price_change(event)?),
                "order" => {
                    if let Some(msg) = self.parse_order(event) {
                        messages.push(msg);
                    }
                }
                "trade" => {
                    if let Some(msg) = self.parse_trade(event) {
                        messages.push(msg);
                    }
                }
                _ => {} // tick_size_change, last_trade_price, etc. — silently ignore
            }
        }
        Ok(messages)
    }

    /// Client sends `"PING"` every ~50s on the user channel; server replies `"PONG"`.
    /// The market channel does not require application-level pings.
    fn build_ping(&self) -> Option<String> {
        Some("PING".to_string())
    }

    fn is_pong(&self, text: &str) -> bool {
        text.trim().eq_ignore_ascii_case("pong")
    }

    fn is_ping(&self, _text: &str) -> bool {
        false
    }
}

impl PolymarketMessageParser {
    // ── Market channel ────────────────────────────────────────────────────────

    fn parse_book(&self, value: &serde_json::Value) -> Result<Vec<StreamMessage>> {
        let event: PolymarketBookEvent = match serde_json::from_value(value.clone()) {
            Ok(e) => e,
            Err(e) => {
                error!("Polymarket: failed to parse book event: {e}");
                return Ok(vec![]);
            }
        };

        let symbol = event.asset_id.clone();
        let ts = parse_timestamp_ms(&event.timestamp);
        let ts_str = ts.to_rfc3339();
        let mut orders = Vec::new();

        for ask in &event.asks {
            let Ok(price) = ask.price.parse::<f64>() else {
                continue;
            };
            let Ok(qty) = ask.size.parse::<f64>() else {
                continue;
            };
            orders.push(GenericOrder {
                price,
                side: "Ask".to_string(),
                qty,
                symbol: symbol.clone(),
                timestamp: ts_str.clone(),
            });
        }

        for bid in &event.bids {
            let Ok(price) = bid.price.parse::<f64>() else {
                continue;
            };
            let Ok(qty) = bid.size.parse::<f64>() else {
                continue;
            };
            orders.push(GenericOrder {
                price,
                side: "Bid".to_string(),
                qty,
                symbol: symbol.clone(),
                timestamp: ts_str.clone(),
            });
        }

        if orders.is_empty() {
            return Ok(vec![]);
        }

        Ok(vec![StreamMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            timestamp: ts,
            exchange: self.exchange_name.clone(),
        })])
    }

    fn parse_price_change(&self, value: &serde_json::Value) -> Result<Vec<StreamMessage>> {
        let event: PolymarketPriceChangeEvent = match serde_json::from_value(value.clone()) {
            Ok(e) => e,
            Err(e) => {
                error!("Polymarket: failed to parse price_change event: {e}");
                return Ok(vec![]);
            }
        };

        let ts = parse_timestamp_ms(&event.timestamp);
        let ts_str = ts.to_rfc3339();

        // Group changes by asset_id — each asset gets its own OrderbookUpdate.
        let mut per_asset: HashMap<String, (OrderbookAction, Vec<GenericOrder>)> = HashMap::new();

        for change in &event.price_changes {
            let Ok(price) = change.price.parse::<f64>() else {
                error!("Polymarket: failed to parse price: {}", change.price);
                continue;
            };
            let Ok(qty) = change.size.parse::<f64>() else {
                error!("Polymarket: failed to parse qty: {}", change.size);
                continue;
            };

            let action = if qty == 0.0 {
                OrderbookAction::Delete
            } else {
                OrderbookAction::Update
            };

            let side = match change.side.as_str() {
                "BUY" => "Bid",
                "SELL" => "Ask",
                other => {
                    warn!("Polymarket: unknown side '{other}', skipping");
                    continue;
                }
            };

            let entry = per_asset
                .entry(change.asset_id.clone())
                .or_insert_with(|| (action.clone(), Vec::new()));

            if matches!(action, OrderbookAction::Delete) {
                entry.0 = OrderbookAction::Delete;
            }

            entry.1.push(GenericOrder {
                price,
                side: side.to_string(),
                qty,
                symbol: change.asset_id.clone(),
                timestamp: ts_str.clone(),
            });
        }

        let messages = per_asset
            .into_iter()
            .filter(|(_, (_, orders))| !orders.is_empty())
            .map(|(symbol, (action, orders))| {
                StreamMessage::OrderbookUpdate(OrderbookUpdate {
                    action,
                    orders,
                    symbol,
                    timestamp: ts,
                    exchange: self.exchange_name.clone(),
                })
            })
            .collect();

        Ok(messages)
    }

    // ── User channel ──────────────────────────────────────────────────────────

    fn parse_order(&self, value: &serde_json::Value) -> Option<StreamMessage> {
        let o: PolyOrderEvent = match serde_json::from_value(value.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("Polymarket: failed to deserialise order event: {e}");
                return None;
            }
        };

        let side = parse_side(&o.side)?;
        let px: f64 = o.price.parse().unwrap_or(0.0);
        let qty: f64 = o.original_size.parse().unwrap_or(0.0);
        let matched: f64 = o.size_matched.parse().unwrap_or(0.0);

        Some(StreamMessage::UserEvent(UserEvent::OrderUpdate {
            exchange: "polymarket".into(),
            client_oid: o.owner, // Polymarket uses owner UUID, no client OID
            exchange_oid: o.id,
            symbol: o.asset_id,
            side,
            px,
            qty,
            filled: matched,
            status: parse_order_type(&o.order_type),
            ts_ns: now_ns(),
        }))
    }

    fn parse_trade(&self, value: &serde_json::Value) -> Option<StreamMessage> {
        let t: PolyTradeEvent = match serde_json::from_value(value.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("Polymarket: failed to deserialise trade event: {e}");
                return None;
            }
        };

        let side = parse_side(&t.side)?;
        let px: f64 = t.price.parse().unwrap_or(0.0);
        let qty: f64 = t.size.parse().unwrap_or(0.0);

        Some(StreamMessage::UserEvent(UserEvent::Fill {
            exchange: "polymarket".into(),
            client_oid: t.taker_order_id,
            exchange_oid: t.id,
            symbol: t.asset_id,
            side,
            px,
            qty,
            fee: 0.0, // Polymarket does not include fee in the trade event
            ts_ns: now_ns(),
        }))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_side(raw: &str) -> Option<Side> {
    match raw.to_uppercase().as_str() {
        "BUY" => Some(Side::Buy),
        "SELL" => Some(Side::Sell),
        other => {
            warn!("Polymarket: unknown side '{other}'");
            None
        }
    }
}

/// Map the `type` field of a Polymarket order event to our `OrderStatus`.
///
/// | Polymarket `type` | Meaning          | Maps to              |
/// |-------------------|------------------|----------------------|
/// | `"PLACEMENT"`     | Order placed      | `New`               |
/// | `"MATCH"`         | Fully matched     | `Filled`            |
/// | `"CANCELLATION"`  | Order cancelled   | `Canceled`          |
/// | `"UPDATE"`        | Partial fill      | `PartiallyFilled`   |
fn parse_order_type(raw: &str) -> OrderStatus {
    match raw.to_uppercase().as_str() {
        "PLACEMENT" => OrderStatus::New,
        "MATCH" => OrderStatus::Filled,
        "CANCELLATION" => OrderStatus::Canceled,
        "UPDATE" => OrderStatus::PartiallyFilled,
        _ => OrderStatus::New,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::{OrderStatus, Side, UserEvent};

    fn parser() -> PolymarketMessageParser {
        PolymarketMessageParser::new()
    }

    #[test]
    fn ignores_unknown_event_type() {
        assert!(
            parser()
                .parse_message(r#"[{"event_type":"last_trade_price"}]"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn handles_empty_ack() {
        assert!(parser().parse_message("[]").unwrap().is_empty());
    }

    #[test]
    fn parses_order_placement_real_wire() {
        let raw = r#"[{
            "asset_id": "52114319501245915516055106046884209969926127482827954674443846427813813222426",
            "associate_trades": null,
            "event_type": "order",
            "id": "0xff354cd7ca7539dfa9c28d90943ab5779a4eac34b9b37a757d7b32bdfb11790b",
            "market": "0xbd31dc8a20211944f6b70f31557f1001557b59905b7738480ca09bd4532f84af",
            "order_owner": "9180014b-33c8-9240-a14b-bdca11c0a465",
            "original_size": "10",
            "outcome": "YES",
            "owner": "9180014b-33c8-9240-a14b-bdca11c0a465",
            "price": "0.57",
            "side": "SELL",
            "size_matched": "0",
            "timestamp": "1672290687",
            "type": "PLACEMENT"
        }]"#;

        let msgs = parser().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let StreamMessage::UserEvent(UserEvent::OrderUpdate {
            exchange_oid,
            symbol,
            side,
            px,
            qty,
            filled,
            status,
            ..
        }) = &msgs[0]
        else {
            panic!("expected UserEvent::OrderUpdate")
        };

        assert_eq!(
            exchange_oid,
            "0xff354cd7ca7539dfa9c28d90943ab5779a4eac34b9b37a757d7b32bdfb11790b"
        );
        assert_eq!(
            symbol,
            "52114319501245915516055106046884209969926127482827954674443846427813813222426"
        );
        assert_eq!(*side, Side::Sell);
        assert!((px - 0.57).abs() < 1e-9);
        assert_eq!(*qty, 10.0);
        assert_eq!(*filled, 0.0); // no fills yet — PLACEMENT
        assert_eq!(*status, OrderStatus::New);
    }

    #[test]
    fn parses_trade_real_wire() {
        let raw = r#"[{
            "asset_id": "52114319501245915516055106046884209969926127482827954674443846427813813222426",
            "event_type": "trade",
            "id": "28c4d2eb-bbea-40e7-a9f0-b2fdb56b2c2e",
            "last_update": "1672290701",
            "maker_orders": [{
                "asset_id": "52114319501245915516055106046884209969926127482827954674443846427813813222426",
                "matched_amount": "10",
                "order_id": "0xff354cd7ca7539dfa9c28d90943ab5779a4eac34b9b37a757d7b32bdfb11790b",
                "outcome": "YES",
                "owner": "9180014b-33c8-9240-a14b-bdca11c0a465",
                "price": "0.57"
            }],
            "market": "0xbd31dc8a20211944f6b70f31557f1001557b59905b7738480ca09bd4532f84af",
            "matchtime": "1672290701",
            "outcome": "YES",
            "owner": "9180014b-33c8-9240-a14b-bdca11c0a465",
            "price": "0.57",
            "side": "BUY",
            "size": "10",
            "status": "MATCHED",
            "taker_order_id": "0x06bc63e346ed4ceddce9efd6b3af37c8f8f440c92fe7da6b2d0f9e4ccbc50c42",
            "timestamp": "1672290701",
            "trade_owner": "9180014b-33c8-9240-a14b-bdca11c0a465",
            "type": "TRADE"
        }]"#;

        let msgs = parser().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let StreamMessage::UserEvent(UserEvent::Fill {
            exchange_oid,
            client_oid,
            symbol,
            side,
            px,
            qty,
            ..
        }) = &msgs[0]
        else {
            panic!("expected UserEvent::Fill")
        };

        assert_eq!(exchange_oid, "28c4d2eb-bbea-40e7-a9f0-b2fdb56b2c2e");
        assert_eq!(
            client_oid,
            "0x06bc63e346ed4ceddce9efd6b3af37c8f8f440c92fe7da6b2d0f9e4ccbc50c42"
        ); // taker_order_id
        assert_eq!(
            symbol,
            "52114319501245915516055106046884209969926127482827954674443846427813813222426"
        );
        assert_eq!(*side, Side::Buy);
        assert!((px - 0.57).abs() < 1e-9);
        assert_eq!(*qty, 10.0);
    }

    #[test]
    fn ping_pong() {
        let p = parser();
        assert_eq!(p.build_ping(), Some("PING".to_string()));
        assert!(p.is_pong("PONG"));
        assert!(p.is_pong("pong"));
        assert!(!p.is_ping("PING"));
    }
}
