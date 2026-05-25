//! Flat record types persisted to CSV.
//!
//! Every record is `Serialize` with no nested fields — the `csv` crate
//! writes one row per record, header row first. Fields that don't apply
//! to a given record (e.g. `fee` on an order_update) are `Option<...>`
//! and serialize as empty cells.

use crate::event::{LastTradePrice, OrderbookSnapshot, UserEvent};
use serde::Serialize;

// ── Snapshots ────────────────────────────────────────────────────────────────

/// One persisted snapshot row.
///
/// Bids and asks are stored as parallel arrays. The `depth` config field
/// controls how many levels we keep. CSV writers expand each level into
/// two columns (`bid0_px`, `bid0_qty`, ...) via `row(...)` — see writer.rs.
/// Msgpack consumers (e.g. `polymarket_recorder` binary) serialize this
/// struct directly via `Serialize`.
#[derive(Serialize)]
pub struct SnapshotRecord {
    pub sequence: u64,
    pub ts_ns: i64,
    pub best_bid_px: Option<f64>,
    pub best_bid_qty: Option<f64>,
    pub best_ask_px: Option<f64>,
    pub best_ask_qty: Option<f64>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    #[serde(skip)]
    pub depth_levels: usize,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

impl SnapshotRecord {
    pub fn from_snapshot(snap: &OrderbookSnapshot, depth: usize) -> Self {
        let best_bid_px = snap.best_bid.map(|(p, _)| p);
        let best_bid_qty = snap.best_bid.map(|(_, q)| q);
        let best_ask_px = snap.best_ask.map(|(p, _)| p);
        let best_ask_qty = snap.best_ask.map(|(_, q)| q);
        let mid = match (best_bid_px, best_ask_px) {
            (Some(b), Some(a)) => Some((b + a) / 2.0),
            _ => None,
        };
        Self {
            sequence: snap.sequence,
            ts_ns: snap.timestamp.timestamp_nanos_opt().unwrap_or(0),
            best_bid_px,
            best_bid_qty,
            best_ask_px,
            best_ask_qty,
            spread: snap.spread,
            mid,
            depth_levels: depth,
            bids: snap.bids[..snap.bids.len().min(depth)].to_vec(),
            asks: snap.asks[..snap.asks.len().min(depth)].to_vec(),
        }
    }

    /// CSV header row for this depth. Caller writes it once per new file.
    pub fn header(depth: usize) -> Vec<String> {
        let mut h = vec![
            "ts_ns".to_string(),
            "sequence".to_string(),
            "best_bid_px".to_string(),
            "best_bid_qty".to_string(),
            "best_ask_px".to_string(),
            "best_ask_qty".to_string(),
            "spread".to_string(),
            "mid".to_string(),
        ];
        for i in 0..depth {
            h.push(format!("bid{i}_px"));
            h.push(format!("bid{i}_qty"));
        }
        for i in 0..depth {
            h.push(format!("ask{i}_px"));
            h.push(format!("ask{i}_qty"));
        }
        h
    }

    /// CSV row as a flat vec of strings. Aligns 1-to-1 with `header(depth)`.
    pub fn row(&self) -> Vec<String> {
        let mut r = vec![
            self.ts_ns.to_string(),
            self.sequence.to_string(),
            fmt_opt(self.best_bid_px),
            fmt_opt(self.best_bid_qty),
            fmt_opt(self.best_ask_px),
            fmt_opt(self.best_ask_qty),
            fmt_opt(self.spread),
            fmt_opt(self.mid),
        ];
        for i in 0..self.depth_levels {
            let (p, q) = self.bids.get(i).copied().unwrap_or((f64::NAN, f64::NAN));
            r.push(fmt_f(p));
            r.push(fmt_f(q));
        }
        for i in 0..self.depth_levels {
            let (p, q) = self.asks.get(i).copied().unwrap_or((f64::NAN, f64::NAN));
            r.push(fmt_f(p));
            r.push(fmt_f(q));
        }
        r
    }
}

// ── Trades ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct TradeRecord {
    /// Unix nanosecond timestamp.
    pub ts_ns: i64,
    pub price: f64,
    pub size: f64,
    /// Taker side: `"BUY"` or `"SELL"`.
    pub side: String,
    pub fee_rate_bps: f64,
    /// Exchange-assigned trade id (Binance `t`); omitted for feeds that
    /// don't carry one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_id: Option<i64>,
}

impl TradeRecord {
    pub fn from_trade(trade: &LastTradePrice) -> Self {
        Self {
            ts_ns: trade.timestamp.timestamp_nanos_opt().unwrap_or(0),
            price: trade.price,
            size: trade.size,
            side: trade.side.clone(),
            fee_rate_bps: trade.fee_rate_bps,
            trade_id: trade.trade_id,
        }
    }
}

// ── User events ─────────────────────────────────────────────────────────────

/// Flat row for any `UserEvent` variant. Unused fields are empty strings.
///
/// The discriminator column `type` ∈ {`fill`, `order_update`} lets a
/// spreadsheet user filter rows. Both variants share most columns; per-
/// variant exclusives (e.g. `fee` on fill, `filled`/`status` on order_update)
/// are `Option<...>` and serialize as empty cells where they don't apply.
///
/// `balance` / `position` are not represented — the Polymarket WS user
/// channel does not push them. If we later add a REST poller for those,
/// expand this struct (or fork a separate record) at that point.
#[derive(Serialize)]
pub struct UserEventRecord {
    pub ts_ns: i64,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub exchange: String,
    pub symbol: String,
    pub side: String,
    pub px: f64,
    pub qty: f64,
    /// Cumulative filled qty (order_update only; empty for fill rows).
    pub filled: Option<f64>,
    /// Lifecycle state (order_update only).
    pub status: Option<String>,
    /// Fee paid (fill only).
    pub fee: Option<f64>,
    /// Taker order id (fill only; empty for order_update rows).
    pub taker_oid: Option<String>,
    /// Client order id when known (order_update always; fill optionally).
    pub client_oid: Option<String>,
    pub exchange_oid: String,
    /// Exchange-specific maker-order list (fill only, Polymarket today).
    /// Serialised as a JSON string so the value fits in one CSV cell.
    pub maker_orders: Option<String>,
}

impl UserEventRecord {
    pub fn from_event(ev: &UserEvent) -> Self {
        match ev {
            UserEvent::Fill {
                exchange,
                taker_oid,
                client_oid,
                exchange_oid,
                symbol,
                side,
                px,
                qty,
                fee,
                ts_ns,
                trade_status,
                maker_orders,
            } => Self {
                ts_ns: *ts_ns,
                kind: "fill",
                exchange: exchange.clone(),
                symbol: symbol.clone(),
                side: format!("{side:?}").to_lowercase(),
                px: *px,
                qty: *qty,
                filled: None,
                status: trade_status.clone(), // MATCHED/MINED/CONFIRMED
                fee: Some(*fee),
                taker_oid: taker_oid.clone(),
                client_oid: client_oid.clone(),
                exchange_oid: exchange_oid.clone(),
                maker_orders: maker_orders
                    .as_ref()
                    .and_then(|v| serde_json::to_string(v).ok()),
            },
            UserEvent::OrderUpdate {
                exchange,
                client_oid,
                exchange_oid,
                symbol,
                side,
                px,
                qty,
                filled,
                status,
                ts_ns,
            } => Self {
                ts_ns: *ts_ns,
                kind: "order_update",
                exchange: exchange.clone(),
                symbol: symbol.clone(),
                side: format!("{side:?}").to_lowercase(),
                px: *px,
                qty: *qty,
                filled: Some(*filled),
                status: Some(format!("{status:?}").to_lowercase()),
                fee: None,
                taker_oid: None,
                client_oid: Some(client_oid.clone()),
                exchange_oid: exchange_oid.clone(),
                maker_orders: None,
            },
        }
    }

    /// Single CSV file under `events/`; `kind` discriminates variants in
    /// the `type` column.
    pub fn dir_name() -> &'static str {
        "events"
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn fmt_opt(x: Option<f64>) -> String {
    match x {
        Some(v) => fmt_f(v),
        None => String::new(),
    }
}

fn fmt_f(x: f64) -> String {
    if x.is_nan() {
        String::new()
    } else {
        // Compact, no trailing zeros, no scientific notation for typical
        // price ranges. `{}` formatting on f64 gives this for us.
        format!("{x}")
    }
}
