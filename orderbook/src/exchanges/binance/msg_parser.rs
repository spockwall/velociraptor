use crate::connection::MessageParserTrait;
use crate::exchanges::binance::types::{BinanceStreamMessage, BinanceSubscribeResponse};
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

        // Parse combined stream message
        let msg: BinanceStreamMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to parse Binance message: {e} - {text}");
                return Ok(vec![]);
            }
        };

        // Extract symbol from stream name, e.g. "btcusdt@depth20@100ms" -> "BTCUSDT"
        let symbol = msg
            .stream
            .split('@')
            .next()
            .unwrap_or("UNKNOWN")
            .to_uppercase();

        let timestamp = Utc::now();
        let ts_str = timestamp.to_rfc3339();

        let mut orders = Vec::new();

        for ask in &msg.data.asks {
            let price: f64 = ask[0].parse().unwrap_or(0.0);
            let qty: f64 = ask[1].parse().unwrap_or(0.0);
            orders.push(GenericOrder {
                id: format!("ask_{price}"),
                price,
                side: "Ask".to_string(),
                qty,
                symbol: symbol.clone(),
                timestamp: ts_str.clone(),
            });
        }

        for bid in &msg.data.bids {
            let price: f64 = bid[0].parse().unwrap_or(0.0);
            let qty: f64 = bid[1].parse().unwrap_or(0.0);
            orders.push(GenericOrder {
                id: format!("bid_{price}"),
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
