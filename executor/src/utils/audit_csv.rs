//! CSV encode + decode helpers for the executor audit log.
//!
//! The audit log is a header-prefixed CSV file at
//! `dir/{YYYY-MM-DD}.csv` with one row per `AuditEntry`. The format is
//! human-readable so an operator can `tail -f` or grep it directly; the
//! variable bits of each entry are packed into the trailing
//! `payload_json` column.
//!
//! Columns:
//!   seq, prev_hash, ts_iso, ts_ns, kind, req_id, op, exchange, payload_json

use libs::protocol::orders::{OrderAction, OrderError, OrderResult};
use serde::Deserialize;

use crate::ops::audit::{AuditEntry, Payload};

/// Header written exactly once per file (when the file is created).
pub const CSV_HEADER: &[u8] =
    b"seq,prev_hash,ts_iso,ts_ns,kind,req_id,op,exchange,payload_json\n";

/// Render `entry` to a single CSV line (terminated with `\n`).
pub fn render_row(entry: &AuditEntry, ts_iso: &str) -> Result<String, csv::Error> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    let (kind, req_id, op, exchange) = match &entry.payload {
        Payload::Request { req } => (
            "request",
            req.req_id.to_string(),
            action_name(&req.action).to_string(),
            req.exchange.to_str().to_string(),
        ),
        Payload::Response { req_id, result } => (
            "response",
            req_id.to_string(),
            result_kind(result).to_string(),
            String::new(),
        ),
        Payload::Synthetic { op, .. } => (
            "synthetic",
            String::new(),
            op.clone(),
            String::new(),
        ),
    };
    let payload_json = serde_json::to_string(&entry.payload)
        .map_err(|e| csv::Error::from(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    wtr.write_record(&[
        entry.seq.to_string(),
        entry.prev_hash.clone(),
        ts_iso.to_string(),
        entry.ts_ns.to_string(),
        kind.to_string(),
        req_id,
        op,
        exchange,
        payload_json,
    ])?;
    let bytes = wtr.into_inner().map_err(|e| {
        csv::Error::from(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))
    })?;
    Ok(String::from_utf8(bytes).expect("csv produces utf-8"))
}

/// Parse every row of an audit CSV byte buffer into `AuditEntry`s.
/// Best-effort: a malformed row ends the scan; earlier rows are returned.
pub fn parse_rows(raw: &[u8]) -> Vec<AuditEntry> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(raw);
    let mut out = Vec::new();
    for row in rdr.deserialize::<CsvRow>() {
        let Ok(row) = row else { break };
        let Ok(payload) = serde_json::from_str::<Payload>(&row.payload_json) else {
            break;
        };
        out.push(AuditEntry {
            seq: row.seq,
            prev_hash: row.prev_hash,
            ts_ns: row.ts_ns,
            payload,
        });
    }
    out
}

#[derive(Debug, Deserialize)]
struct CsvRow {
    seq: u64,
    prev_hash: String,
    #[allow(dead_code)]
    ts_iso: String,
    ts_ns: i64,
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    req_id: String,
    #[allow(dead_code)]
    op: String,
    #[allow(dead_code)]
    exchange: String,
    payload_json: String,
}

fn action_name(a: &OrderAction) -> &'static str {
    use OrderAction::*;
    match a {
        Place(_) => "place",
        PlaceBatch { .. } => "place_batch",
        Update { .. } => "update",
        Cancel { .. } => "cancel",
        CancelAll => "cancel_all",
        CancelMarket { .. } => "cancel_market",
        Heartbeat => "heartbeat",
    }
}

fn result_kind(r: &Result<OrderResult, OrderError>) -> &'static str {
    match r {
        Ok(OrderResult::Ack(_)) => "ack",
        Ok(OrderResult::BatchAck { .. }) => "batch_ack",
        Ok(OrderResult::CancelCount { .. }) => "cancel_count",
        Ok(OrderResult::HeartbeatOk(_)) => "heartbeat_ok",
        Err(OrderError::RiskRejected { .. }) => "risk_rejected",
        Err(OrderError::KillSwitch) => "kill_switch",
        Err(OrderError::DuplicateClientOid { .. }) => "duplicate_client_oid",
        Err(OrderError::ExchangeRejected { .. }) => "exchange_rejected",
        Err(OrderError::Network { .. }) => "network",
        Err(OrderError::Timeout) => "timeout",
        Err(OrderError::NotFound { .. }) => "not_found",
        Err(OrderError::Internal { .. }) => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::orders::{
        OrderAck, OrderKind, OrderRequest, OrderStatus, PlaceOne, Side, Tif,
    };
    use libs::protocol::ExchangeName;

    fn entry_request(seq: u64) -> AuditEntry {
        AuditEntry {
            seq,
            prev_hash: "0".repeat(64),
            ts_ns: 1_700_000_000_000_000_000,
            payload: Payload::Request {
                req: OrderRequest {
                    req_id: 42,
                    exchange: ExchangeName::Kalshi,
                    action: OrderAction::Place(PlaceOne {
                        client_oid: "c-1".into(),
                        symbol: "X.YES".into(),
                        side: Side::Buy,
                        kind: OrderKind::Limit,
                        px: 0.5,
                        qty: 1.0,
                        tif: Tif::Gtc,
                    }),
                },
            },
        }
    }

    fn entry_response_ack(seq: u64) -> AuditEntry {
        AuditEntry {
            seq,
            prev_hash: "1".repeat(64),
            ts_ns: 1_700_000_000_000_000_001,
            payload: Payload::Response {
                req_id: 42,
                result: Ok(OrderResult::Ack(OrderAck {
                    client_oid: "c-1".into(),
                    exchange_oid: "ex-1".into(),
                    status: OrderStatus::New,
                    ts_ns: 0,
                })),
            },
        }
    }

    fn entry_response_err(seq: u64) -> AuditEntry {
        AuditEntry {
            seq,
            prev_hash: "2".repeat(64),
            ts_ns: 1_700_000_000_000_000_002,
            payload: Payload::Response {
                req_id: 43,
                result: Err(OrderError::RiskRejected {
                    rule: "max_qty".into(),
                    detail: "qty too high".into(),
                }),
            },
        }
    }

    fn entry_synthetic(seq: u64) -> AuditEntry {
        AuditEntry {
            seq,
            prev_hash: "3".repeat(64),
            ts_ns: 1_700_000_000_000_000_003,
            payload: Payload::Synthetic {
                op: "cancel_all_fired".into(),
                detail: serde_json::json!({ "exchange": "kalshi", "count": 7 }),
            },
        }
    }

    #[test]
    fn header_matches_columns() {
        let s = String::from_utf8(CSV_HEADER.to_vec()).unwrap();
        assert_eq!(
            s.trim_end(),
            "seq,prev_hash,ts_iso,ts_ns,kind,req_id,op,exchange,payload_json"
        );
    }

    #[test]
    fn render_request_fills_label_columns() {
        let line = render_row(&entry_request(0), "2026-05-25T00:00:00Z").unwrap();
        // Trailing newline is included.
        assert!(line.ends_with('\n'));
        let row = line.trim_end_matches('\n');
        let cols: Vec<&str> = row.splitn(9, ',').collect();
        assert_eq!(cols[0], "0"); // seq
        assert_eq!(cols[2], "2026-05-25T00:00:00Z"); // ts_iso
        assert_eq!(cols[4], "request"); // kind
        assert_eq!(cols[5], "42"); // req_id
        assert_eq!(cols[6], "place"); // op
        assert_eq!(cols[7], "kalshi"); // exchange
        // payload_json starts with a quote because csv quoted the JSON.
        assert!(cols[8].starts_with('"'));
    }

    #[test]
    fn render_response_ack_uses_ack_op_no_exchange() {
        let line = render_row(&entry_response_ack(1), "t").unwrap();
        let cols: Vec<&str> = line.trim_end().splitn(9, ',').collect();
        assert_eq!(cols[4], "response");
        assert_eq!(cols[5], "42");
        assert_eq!(cols[6], "ack");
        assert_eq!(cols[7], ""); // exchange empty for response rows
    }

    #[test]
    fn render_response_err_uses_error_kind() {
        let line = render_row(&entry_response_err(2), "t").unwrap();
        let cols: Vec<&str> = line.trim_end().splitn(9, ',').collect();
        assert_eq!(cols[6], "risk_rejected");
    }

    #[test]
    fn render_synthetic_leaves_req_id_and_exchange_blank() {
        let line = render_row(&entry_synthetic(3), "t").unwrap();
        let cols: Vec<&str> = line.trim_end().splitn(9, ',').collect();
        assert_eq!(cols[4], "synthetic");
        assert_eq!(cols[5], ""); // req_id blank
        assert_eq!(cols[6], "cancel_all_fired");
        assert_eq!(cols[7], "");
    }

    #[test]
    fn round_trip_preserves_entries() {
        let entries = vec![
            entry_request(0),
            entry_response_ack(1),
            entry_response_err(2),
            entry_synthetic(3),
        ];
        // Build a buffer: header + every row.
        let mut buf: Vec<u8> = CSV_HEADER.to_vec();
        for e in &entries {
            let line = render_row(e, "t").unwrap();
            buf.extend_from_slice(line.as_bytes());
        }
        let parsed = parse_rows(&buf);
        assert_eq!(parsed.len(), entries.len());
        for (a, b) in entries.iter().zip(parsed.iter()) {
            assert_eq!(a.seq, b.seq);
            assert_eq!(a.prev_hash, b.prev_hash);
            assert_eq!(a.ts_ns, b.ts_ns);
            // Compare payload via JSON, since `Payload` doesn't impl PartialEq.
            let lhs = serde_json::to_string(&a.payload).unwrap();
            let rhs = serde_json::to_string(&b.payload).unwrap();
            assert_eq!(lhs, rhs);
        }
    }

    #[test]
    fn payload_with_commas_and_quotes_round_trips() {
        let entry = AuditEntry {
            seq: 99,
            prev_hash: "f".repeat(64),
            ts_ns: 0,
            payload: Payload::Synthetic {
                op: "trick".into(),
                detail: serde_json::json!({
                    "note": "has, a comma and \"quotes\"",
                    "nested": [1, 2, 3],
                }),
            },
        };
        let mut buf = CSV_HEADER.to_vec();
        buf.extend_from_slice(render_row(&entry, "t").unwrap().as_bytes());
        let parsed = parse_rows(&buf);
        assert_eq!(parsed.len(), 1);
        match &parsed[0].payload {
            Payload::Synthetic { op, detail } => {
                assert_eq!(op, "trick");
                assert_eq!(
                    detail["note"].as_str().unwrap(),
                    "has, a comma and \"quotes\""
                );
                assert_eq!(detail["nested"][2], 3);
            }
            _ => panic!("expected synthetic"),
        }
    }

    #[test]
    fn parse_rows_truncates_at_malformed_payload() {
        let good = render_row(&entry_request(0), "t").unwrap();
        let mut buf = CSV_HEADER.to_vec();
        buf.extend_from_slice(good.as_bytes());
        // Append a row with valid CSV shape but invalid payload_json.
        buf.extend_from_slice(
            b"1,deadbeef,t,0,synthetic,,bad,,not-json-at-all\n",
        );
        // And a third otherwise-valid row that we must NOT see (scan stops
        // at the first malformed entry).
        buf.extend_from_slice(render_row(&entry_synthetic(2), "t").unwrap().as_bytes());
        let parsed = parse_rows(&buf);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].seq, 0);
    }

    #[test]
    fn parse_rows_empty_input_returns_empty() {
        assert!(parse_rows(b"").is_empty());
        // Header-only file.
        assert!(parse_rows(CSV_HEADER).is_empty());
    }

    #[test]
    fn render_row_action_names_cover_all_variants() {
        use OrderAction::*;
        let cases: &[(OrderAction, &str)] = &[
            (
                Place(PlaceOne {
                    client_oid: "x".into(),
                    symbol: "S".into(),
                    side: Side::Buy,
                    kind: OrderKind::Limit,
                    px: 0.1,
                    qty: 1.0,
                    tif: Tif::Gtc,
                }),
                "place",
            ),
            (PlaceBatch { orders: vec![] }, "place_batch"),
            (
                Update {
                    client_oid: "x".into(),
                    exchange_oid: "e".into(),
                    new_px: None,
                    new_qty: None,
                },
                "update",
            ),
            (
                Cancel {
                    exchange_oid: "e".into(),
                },
                "cancel",
            ),
            (CancelAll, "cancel_all"),
            (
                CancelMarket {
                    symbol: "S".into(),
                },
                "cancel_market",
            ),
            (Heartbeat, "heartbeat"),
        ];
        for (action, want_op) in cases {
            let entry = AuditEntry {
                seq: 0,
                prev_hash: "0".repeat(64),
                ts_ns: 0,
                payload: Payload::Request {
                    req: OrderRequest {
                        req_id: 1,
                        exchange: ExchangeName::Kalshi,
                        action: action.clone(),
                    },
                },
            };
            let line = render_row(&entry, "t").unwrap();
            let cols: Vec<&str> = line.trim_end().splitn(9, ',').collect();
            assert_eq!(cols[6], *want_op, "action {:?}", action);
        }
    }

    #[test]
    fn render_row_result_kinds_cover_all_variants() {
        use libs::protocol::orders::HeartbeatAck;
        let ok_ack = OrderResult::Ack(OrderAck {
            client_oid: "x".into(),
            exchange_oid: "e".into(),
            status: OrderStatus::New,
            ts_ns: 0,
        });
        let cases: Vec<(Result<OrderResult, OrderError>, &str)> = vec![
            (Ok(ok_ack), "ack"),
            (Ok(OrderResult::BatchAck { results: vec![] }), "batch_ack"),
            (Ok(OrderResult::CancelCount { count: 0 }), "cancel_count"),
            (
                Ok(OrderResult::HeartbeatOk(HeartbeatAck { next_due_ms: 0 })),
                "heartbeat_ok",
            ),
            (
                Err(OrderError::RiskRejected {
                    rule: "r".into(),
                    detail: "d".into(),
                }),
                "risk_rejected",
            ),
            (Err(OrderError::KillSwitch), "kill_switch"),
            (
                Err(OrderError::DuplicateClientOid {
                    client_oid: "x".into(),
                }),
                "duplicate_client_oid",
            ),
            (
                Err(OrderError::ExchangeRejected {
                    code: None,
                    message: "m".into(),
                }),
                "exchange_rejected",
            ),
            (
                Err(OrderError::Network {
                    message: "m".into(),
                }),
                "network",
            ),
            (Err(OrderError::Timeout), "timeout"),
            (
                Err(OrderError::NotFound {
                    exchange_oid: "e".into(),
                }),
                "not_found",
            ),
            (
                Err(OrderError::Internal {
                    message: "m".into(),
                }),
                "internal",
            ),
        ];
        for (result, want_op) in cases {
            let entry = AuditEntry {
                seq: 0,
                prev_hash: "0".repeat(64),
                ts_ns: 0,
                payload: Payload::Response {
                    req_id: 1,
                    result,
                },
            };
            let line = render_row(&entry, "t").unwrap();
            let cols: Vec<&str> = line.trim_end().splitn(9, ',').collect();
            assert_eq!(cols[6], want_op);
        }
    }
}
