use serde::{Deserialize, Serialize};

use crate::protocol::ExchangeName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKind {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Tif {
    Gtc,
    Ioc,
    Fok,
    Gtd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceOne {
    pub client_oid: String,
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    pub px: f64,
    pub qty: f64,
    pub tif: Tif,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OrderAction {
    Place(PlaceOne),
    PlaceBatch {
        orders: Vec<PlaceOne>,
    },
    Update {
        client_oid: String,
        exchange_oid: String,
        new_px: Option<f64>,
        new_qty: Option<f64>,
    },
    Cancel {
        exchange_oid: String,
    },
    CancelAll,
    CancelMarket {
        symbol: String,
    },
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderRequest {
    pub req_id: u64,
    pub exchange: ExchangeName,
    pub action: OrderAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_oid: String,
    pub exchange_oid: String,
    pub status: OrderStatus,
    pub ts_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatAck {
    pub next_due_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrderError {
    RiskRejected {
        rule: String,
        detail: String,
    },
    KillSwitch,
    DuplicateClientOid {
        client_oid: String,
    },
    ExchangeRejected {
        code: Option<String>,
        message: String,
    },
    Network {
        message: String,
    },
    Timeout,
    NotFound {
        exchange_oid: String,
    },
    Internal {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum OrderResult {
    Ack(OrderAck),
    BatchAck {
        results: Vec<Result<OrderAck, OrderError>>,
    },
    CancelCount {
        count: u32,
    },
    HeartbeatOk(HeartbeatAck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderResponse {
    pub req_id: u64,
    pub result: Result<OrderResult, OrderError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let bytes = rmp_serde::to_vec_named(v).expect("encode");
        let decoded: T = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(v, &decoded);
    }

    #[test]
    fn roundtrip_place() {
        let req = OrderRequest {
            req_id: 1,
            exchange: ExchangeName::Polymarket,
            action: OrderAction::Place(PlaceOne {
                client_oid: "c1".into(),
                symbol: "TRUMP-2028".into(),
                side: Side::Buy,
                kind: OrderKind::Limit,
                px: 0.42,
                qty: 100.0,
                tif: Tif::Gtc,
            }),
        };
        roundtrip(&req);
    }

    #[test]
    fn roundtrip_heartbeat_and_cancelall() {
        roundtrip(&OrderRequest {
            req_id: 2,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Heartbeat,
        });
        roundtrip(&OrderRequest {
            req_id: 3,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::CancelAll,
        });
    }

    #[test]
    fn roundtrip_response_variants() {
        let ok = OrderResponse {
            req_id: 1,
            result: Ok(OrderResult::Ack(OrderAck {
                client_oid: "c1".into(),
                exchange_oid: "x1".into(),
                status: OrderStatus::New,
                ts_ns: 1_700_000_000_000_000_000,
            })),
        };
        roundtrip(&ok);

        let err = OrderResponse {
            req_id: 2,
            result: Err(OrderError::KillSwitch),
        };
        roundtrip(&err);

        let batch = OrderResponse {
            req_id: 3,
            result: Ok(OrderResult::BatchAck {
                results: vec![
                    Ok(OrderAck {
                        client_oid: "c1".into(),
                        exchange_oid: "x1".into(),
                        status: OrderStatus::New,
                        ts_ns: 1,
                    }),
                    Err(OrderError::Timeout),
                ],
            }),
        };
        roundtrip(&batch);
    }
}
