//! Record types persisted by the recorder.
//!
//! - `SnapshotRecord` / `TradeRecord` are serialized to **MessagePack** by
//!   `writer.rs` (and by `polymarket_recorder`) via `rmp_serde::to_vec_named`,
//!   one length-prefixed frame per record. They carry no `exchange`/`symbol` —
//!   those are encoded in the file path.
//! - `UserEventRecord` is the **CSV** row for the low-volume user-event stream;
//!   it's flat with no nested fields, and per-variant exclusives (e.g. `fee` on
//!   a fill, `filled`/`status` on an order_update) are `Option<...>` so they
//!   serialize as empty cells where they don't apply.

use crate::event::{LastTradePrice, OrderbookSnapshot, UserEvent};
use serde::Serialize;

// ── Snapshots ────────────────────────────────────────────────────────────────

/// One persisted snapshot record (MessagePack).
///
/// Bids and asks are stored as parallel `[price, qty]` arrays, best first; the
/// `depth` argument to `from_snapshot` caps how many levels are kept. Derived
/// metrics (`best_bid_px`, `spread`, `mid`, `wmid`) are intentionally NOT
/// stored — they're recomputed at read time from `bids`/`asks` — so the
/// on-disk record stays small. `exchange`/`symbol` live in the file path.
#[derive(Serialize)]
pub struct SnapshotRecord {
    pub sequence: u64,
    pub ts_ns: i64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

impl SnapshotRecord {
    pub fn from_snapshot(snap: &OrderbookSnapshot, depth: usize) -> Self {
        Self {
            sequence: snap.sequence,
            ts_ns: snap.timestamp.timestamp_nanos_opt().unwrap_or(0),
            bids: snap.bids[..snap.bids.len().min(depth)].to_vec(),
            asks: snap.asks[..snap.asks.len().min(depth)].to_vec(),
        }
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
    /// Exchange-side order id. `None` for Polymarket fills (the venue
    /// doesn't expose an order id on trade events — match via
    /// `taker_oid` + `maker_orders` instead).
    pub exchange_oid: Option<String>,
    /// Venue-assigned trade id (fill only; Polymarket emits a UUID).
    pub trade_id: Option<String>,
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
                trade_id,
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
                trade_id: trade_id.clone(),
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
                exchange_oid: Some(exchange_oid.clone()),
                trade_id: None,
                maker_orders: None,
            },
        }
    }
}
