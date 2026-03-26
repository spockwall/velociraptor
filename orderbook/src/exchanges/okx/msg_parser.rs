use crate::connection::{BasicConnectionMsgTrait, MessageParserTrait};
use crate::types::ExchangeName;
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookMessage, OrderbookUpdate};
use anyhow::{Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OkxMessage {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    arg: Option<OkxArg>,
    #[serde(default)]
    data: Option<Vec<Value>>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OkxArg {
    channel: String,
    #[serde(rename = "instId")]
    inst_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OkxOrderBookData {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    checksum: Option<i64>,
    #[serde(rename = "prevSeqId")]
    prev_seq_id: Option<i64>,
    #[serde(rename = "seqId")]
    seq_id: Option<i64>,
    ts: String,
}

pub struct OkxMessageParser {
    exchange_name: ExchangeName,
}

impl OkxMessageParser {
    pub fn new() -> Self {
        Self {
            exchange_name: ExchangeName::Okx,
        }
    }
}

impl Default for OkxMessageParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageParserTrait<OrderbookMessage> for OkxMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<OrderbookMessage>> {
        let mut messages = Vec::new();

        // Parse JSON message
        let msg: OkxMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to parse message as OKX format: {e} - {text}");
                return Ok(messages);
            }
        };

        // Handle subscription confirmations
        if let Some(event) = &msg.event {
            match event.as_str() {
                "subscribe" => {
                    info!("OKX subscription confirmed");
                    return Ok(messages);
                }
                "error" => {
                    let error_msg = msg.msg.unwrap_or_else(|| "Unknown OKX error".to_string());
                    error!("OKX error: {error_msg}");
                    messages.push(OrderbookMessage::error(error_msg));
                    return Ok(messages);
                }
                "login" => {
                    info!("OKX login response received");
                    return Ok(messages);
                }
                "channel-conn-count" => {
                    info!("OKX channel-conn-count received");
                    return Ok(messages);
                }
                _ => warn!("OKX unknown event: {event}"),
            }
        }

        // Handle orderbook data
        if let Some(arg) = &msg.arg {
            if arg.channel == "books" {
                if let Ok(update) = self.parse_orderbook_message(&msg) {
                    messages.push(OrderbookMessage::OrderbookUpdate(update));
                }
            }
        }

        Ok(messages)
    }

    fn build_ping(&self) -> Option<String> {
        Some("ping".to_string())
    }
}

impl OkxMessageParser {
    fn parse_orderbook_message(&self, msg: &OkxMessage) -> Result<OrderbookUpdate> {
        let action = match msg.action.as_deref() {
            Some("snapshot") => OrderbookAction::Snapshot,
            Some("update") => OrderbookAction::Update,
            _ => {
                return Err(anyhow!("Unknown OKX orderbook action: {:?}", msg.action));
            }
        };

        let data = match &msg.data {
            Some(data) if !data.is_empty() => &data[0],
            _ => {
                return Err(anyhow!("No data in OKX orderbook message"));
            }
        };

        let orderbook_data: OkxOrderBookData = serde_json::from_value(data.clone())?;
        let symbol = msg
            .arg
            .as_ref()
            .map(|arg| arg.inst_id.clone())
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let mut orders = Vec::new();

        // Parse asks
        for ask in &orderbook_data.asks {
            if ask.len() >= 4 {
                let price: f64 = ask[0].parse().unwrap_or(0.0);
                let qty: f64 = ask[1].parse().unwrap_or(0.0);

                orders.push(GenericOrder {
                    price,
                    side: "Ask".to_string(),
                    qty,
                    symbol: symbol.clone(),
                    timestamp: orderbook_data.ts.clone(),
                });
            } else {
                error!("Invalid ask length: {:?}", ask);
            }
        }

        // Parse bids
        for bid in &orderbook_data.bids {
            if bid.len() >= 4 {
                let price: f64 = bid[0].parse().unwrap_or(0.0);
                let qty: f64 = bid[1].parse().unwrap_or(0.0);

                orders.push(GenericOrder {
                    price,
                    side: "Bid".to_string(),
                    qty,
                    symbol: symbol.clone(),
                    timestamp: orderbook_data.ts.clone(),
                });
            } else {
                error!("Invalid bid length: {:?}", bid);
            }
        }

        if orders.is_empty() {
            return Err(anyhow!("No valid orders in OKX message"));
        }

        Ok(OrderbookUpdate {
            action,
            orders,
            symbol,
            timestamp: Utc::now(),
            exchange: self.exchange_name.clone(),
        })
    }
}
