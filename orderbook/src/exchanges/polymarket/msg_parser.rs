use crate::connection::MessageParserTrait;
use crate::exchanges::polymarket::types::{PolymarketBookEvent, PolymarketPriceChangeEvent};
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookMessage, OrderbookUpdate};
use anyhow::Result;
use libs::protocol::ExchangeName;
use libs::time::parse_timestamp_ms;
use std::collections::HashMap;
use tracing::{error, warn};

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

impl MessageParserTrait<OrderbookMessage> for PolymarketMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<OrderbookMessage>> {
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
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
            match event.get("event_type").and_then(|v| v.as_str()) {
                Some("book") => messages.extend(self.parse_book(event)?),
                Some("price_change") => messages.extend(self.parse_price_change(event)?),
                Some(_) => {}
                None => {
                    warn!("Polymarket: event missing event_type — {}", event);
                }
            }
        }
        Ok(messages)
    }
}

impl PolymarketMessageParser {
    fn parse_book(&self, value: &serde_json::Value) -> Result<Vec<OrderbookMessage>> {
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

        Ok(vec![OrderbookMessage::OrderbookUpdate(OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            timestamp: ts,
            exchange: self.exchange_name.clone(),
        })])
    }

    fn parse_price_change(&self, value: &serde_json::Value) -> Result<Vec<OrderbookMessage>> {
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

            // If any level in this batch is a Delete, keep the most recent action.
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
                OrderbookMessage::OrderbookUpdate(OrderbookUpdate {
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
}
