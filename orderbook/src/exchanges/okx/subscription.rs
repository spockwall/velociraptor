/// see: https://www.okx.com/docs-v5/en/#trading-account-websocket-positions-channel
/// see:https://www.okx.com/docs-v5/en/#order-book-trading-trade-ws-order-channel
use serde_json::Value;

pub struct OkxSubMsgBuilder {
    pub args: Vec<Value>,
}

impl OkxSubMsgBuilder {
    pub fn new() -> Self {
        Self { args: vec![] }
    }

    pub fn build(&self) -> String {
        serde_json::json!({
            "op": "subscribe",
            "args": self.args
        })
        .to_string()
    }

    pub fn with_position_channel(mut self, inst_id: Option<&str>, inst_type: &str) -> Self {
        if inst_id.is_none() {
            self.args.push(serde_json::json!({
                "channel": "positions",
                "instType": inst_type,
            }));
        }
        self.args.push(serde_json::json!({
            "channel": "positions",
            "instType": inst_type,
            "inst_id": inst_id
        }));
        self
    }

    pub fn with_orders_channel(mut self, inst_id: Option<&str>, inst_type: &str) -> Self {
        if inst_id.is_none() {
            self.args.push(serde_json::json!({
                "channel": "orders",
                "instType": inst_type,
            }));
        }
        self.args.push(serde_json::json!({
            "channel": "orders",
            "instType": inst_type,
            "inst_id": inst_id
        }));
        self
    }

    pub fn with_ticker_channel(mut self, inst_id: &str) -> Self {
        self.args.push(serde_json::json!({
            "channel": "tickers",
            "instId": inst_id
        }));
        self
    }

    pub fn with_orderbook_channel(mut self, inst_id: &str, inst_type: &str) -> Self {
        self.args.push(serde_json::json!({
            "channel": "books",
            "instId": inst_id,
            "instType": inst_type.to_uppercase()
        }));
        self
    }

    pub fn with_orderbook_channel_multi(mut self, inst_ids: Vec<&str>, inst_type: &str) -> Self {
        for inst_id in inst_ids {
            self.args.push(serde_json::json!({
                "channel": "books",
                "instId": inst_id,
                "instType": inst_type.to_uppercase()
            }));
        }
        self
    }
}
