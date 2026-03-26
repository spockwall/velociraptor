use crate::connection::MessageParserTrait;
use crate::exchanges::binance::types::{BinanceDepthData, BinanceSubscribeResponse};
use crate::types::ExchangeName;
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookMessage, OrderbookUpdate};
use anyhow::Result;
use chrono::Utc;
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
}

impl Default for BinanceMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageParserTrait<OrderbookMessage> for BinanceMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<OrderbookMessage>> {
        // Handle subscription confirmation: {"result":null,"id":1}
        if let Ok(resp) = serde_json::from_str::<BinanceSubscribeResponse>(text) {
            if resp.id.is_some() {
                info!("Binance subscription confirmed");
                return Ok(vec![]);
            }
        }

        // Parse raw partial depth message from fstream.binance.com/ws
        let msg: BinanceDepthData = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to parse Binance message: {e} - {text}");
                return Ok(vec![]);
            }
        };

        let symbol = msg.symbol.to_uppercase();
        let timestamp = Utc::now();
        let ts_str = timestamp.to_rfc3339();

        let mut orders = Vec::new();

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
            return Ok(vec![]);
        }

        // Binance depth20 is a snapshot
        let update = OrderbookUpdate {
            action: OrderbookAction::Snapshot,
            orders,
            symbol,
            timestamp,
            exchange: self.exchange_name.clone(),
        };

        Ok(vec![OrderbookMessage::OrderbookUpdate(update)])
    }

    // Binance uses protocol-level WebSocket ping frames — no application-level ping needed
    fn build_ping(&self) -> Option<String> {
        None
    }

    fn is_pong(&self, _text: &str) -> bool {
        false
    }
}
