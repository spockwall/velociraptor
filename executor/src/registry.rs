//! `OrderRegistry` — single source of truth for "what did the executor place".
//!
//! Subsumes the old idempotency cache and feeds the boot-time reconcile.
//!
//! - **Idempotency replay**: a `Place` arriving with a previously-seen
//!   `client_oid` returns the prior `OrderResponse` (subject to TTL —
//!   60s for Place, 10s for Cancel/Update), without re-hitting the
//!   exchange. Protects against engine-side retries on timeout.
//! - **Reconcile known-set**: at boot the registry is rehydrated from the
//!   on-disk audit mpack; `live_set(exchange)` returns the exchange_oids
//!   the executor *thinks* should still exist, which `ops::reconcile_one`
//!   diff's against `client.get_orders()`.
//! - **Runtime status updates**: `mark_terminal(client_oid, status)` is
//!   invoked when an order-update WS event arrives for one of our orders.
//!   Terminal entries stay in the registry until eviction; they just
//!   stop counting in `live_set`.
//!
//! In-memory only, soft-capped. On capacity overflow the oldest insert
//! is evicted (FIFO). TTL expiry is checked lazily on `lookup_idem`.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use libs::protocol::orders::{
    OrderAction, OrderAck, OrderError, OrderRequest, OrderResponse, OrderResult, OrderStatus,
    PlaceOne,
};
use libs::protocol::ExchangeName;

const DEFAULT_CAP: usize = 10_000;
pub const PLACE_TTL: Duration = Duration::from_secs(60);
pub const CANCEL_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdemKind {
    Place,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct OrderEntry {
    pub exchange: ExchangeName,
    pub symbol: Option<String>,
    pub response: OrderResponse,
    pub exchange_oid: Option<String>,
    pub status: OrderStatus,
    pub placed_at: Instant,
    pub kind: IdemKind,
    pub idem_expires_at: Instant,
}

pub struct OrderRegistry {
    by_client_oid: DashMap<String, OrderEntry>,
    /// FIFO of client_oids in insertion order — for capacity-based eviction.
    insertion_order: Mutex<VecDeque<String>>,
    capacity: usize,
}

impl Default for OrderRegistry {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAP)
    }
}

impl OrderRegistry {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            by_client_oid: DashMap::new(),
            insertion_order: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
            capacity: capacity.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.by_client_oid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_client_oid.is_empty()
    }

    /// Idempotency lookup: returns the prior `OrderResponse` for
    /// `client_oid` if one exists and has not expired. Expired entries are
    /// evicted as a side effect.
    pub fn lookup_idem(&self, client_oid: &str) -> Option<OrderResponse> {
        if let Some(entry) = self.by_client_oid.get(client_oid) {
            if Instant::now() < entry.idem_expires_at {
                return Some(entry.response.clone());
            }
        } else {
            return None;
        }
        // Expired — drop the entry.
        self.by_client_oid.remove(client_oid);
        None
    }

    /// Record the result of a Place or Cancel dispatch.
    pub fn record(&self, req: &OrderRequest, response: &OrderResponse) {
        let (client_oid, symbol, kind) = match &req.action {
            OrderAction::Place(p) => (p.client_oid.clone(), Some(p.symbol.clone()), IdemKind::Place),
            OrderAction::PlaceBatch { orders } => {
                for leg in orders {
                    self.insert_entry(OrderEntry {
                        exchange: req.exchange,
                        symbol: Some(leg.symbol.clone()),
                        response: response.clone(),
                        exchange_oid: exchange_oid_for(&response.result, &leg.client_oid),
                        status: OrderStatus::New,
                        placed_at: Instant::now(),
                        kind: IdemKind::Place,
                        idem_expires_at: Instant::now() + PLACE_TTL,
                    }, leg.client_oid.clone());
                }
                return;
            }
            OrderAction::Cancel { exchange_oid } => {
                // No client_oid for a Cancel; key the registry on the
                // exchange_oid so a retried cancel finds the prior result.
                (exchange_oid.clone(), None, IdemKind::Cancel)
            }
            OrderAction::Update { client_oid, .. } => {
                (client_oid.clone(), None, IdemKind::Cancel)
            }
            // CancelAll / CancelMarket / Heartbeat aren't idempotency-keyed.
            _ => return,
        };
        let ttl = match kind {
            IdemKind::Place => PLACE_TTL,
            IdemKind::Cancel => CANCEL_TTL,
        };
        let now = Instant::now();
        self.insert_entry(
            OrderEntry {
                exchange: req.exchange,
                symbol,
                response: response.clone(),
                exchange_oid: exchange_oid_for(&response.result, &client_oid),
                status: starting_status(&response.result),
                placed_at: now,
                kind,
                idem_expires_at: now + ttl,
            },
            client_oid,
        );
    }

    fn insert_entry(&self, entry: OrderEntry, client_oid: String) {
        // Capacity-based FIFO eviction.
        {
            let mut order = self.insertion_order.lock().unwrap();
            if order.len() >= self.capacity {
                if let Some(old) = order.pop_front() {
                    self.by_client_oid.remove(&old);
                }
            }
            order.push_back(client_oid.clone());
        }
        self.by_client_oid.insert(client_oid, entry);
    }

    /// Mark an existing entry terminal (filled / canceled / rejected /
    /// expired). No-op if `client_oid` isn't in the registry.
    pub fn mark_terminal(&self, client_oid: &str, status: OrderStatus) {
        if let Some(mut e) = self.by_client_oid.get_mut(client_oid) {
            e.status = status;
        }
    }

    /// `exchange_oid`s the executor thinks are still live for `exchange`.
    /// Consumed by boot-time reconcile.
    pub fn live_set(&self, exchange: ExchangeName) -> HashSet<String> {
        self.by_client_oid
            .iter()
            .filter(|kv| kv.value().exchange == exchange && is_live(kv.value().status))
            .filter_map(|kv| kv.value().exchange_oid.clone())
            .collect()
    }

    /// Drop every entry for `exchange` (called after a successful
    /// cancel-all flatten). Idempotency TTL still applies to any new
    /// Place that arrives afterward.
    pub fn reset_for_exchange(&self, exchange: ExchangeName) {
        let to_drop: Vec<String> = self
            .by_client_oid
            .iter()
            .filter(|kv| kv.value().exchange == exchange)
            .map(|kv| kv.key().clone())
            .collect();
        for k in &to_drop {
            self.by_client_oid.remove(k);
        }
        let mut order = self.insertion_order.lock().unwrap();
        order.retain(|k| self.by_client_oid.contains_key(k));
    }

    /// Helper for tests / boot: directly seed an entry built externally.
    #[cfg(test)]
    pub(crate) fn seed(&self, client_oid: String, entry: OrderEntry) {
        self.insert_entry(entry, client_oid);
    }

    /// Replay today's audit log and seed the registry with every
    /// Place / PlaceBatch we see followed by its Response. Best-effort.
    /// Entries get an `idem_expires_at` of *now* + TTL — slightly
    /// generous, but the alternative is dropping idempotency entirely on
    /// restart, which is worse.
    pub async fn rehydrate_from_audit(&self, audit_dir: &std::path::Path) {
        use crate::ops::audit::Payload;
        let entries = crate::ops::AuditSink::replay_today(audit_dir).await;
        let mut pending: std::collections::HashMap<u64, OrderRequest> =
            std::collections::HashMap::new();
        for e in entries {
            match e.payload {
                Payload::Request { req } => {
                    pending.insert(req.req_id, req);
                }
                Payload::Response { req_id, result } => {
                    let Some(req) = pending.remove(&req_id) else {
                        continue;
                    };
                    let resp = OrderResponse {
                        req_id,
                        result,
                    };
                    self.record(&req, &resp);
                }
                Payload::Synthetic { .. } => {}
            }
        }
    }
}

fn is_live(s: OrderStatus) -> bool {
    matches!(s, OrderStatus::New | OrderStatus::PartiallyFilled)
}

fn starting_status(result: &Result<OrderResult, OrderError>) -> OrderStatus {
    match result {
        Ok(OrderResult::Ack(a)) => a.status,
        Ok(_) => OrderStatus::New,
        Err(_) => OrderStatus::Rejected,
    }
}

fn exchange_oid_for(
    result: &Result<OrderResult, OrderError>,
    client_oid: &str,
) -> Option<String> {
    match result {
        Ok(OrderResult::Ack(a)) => Some(a.exchange_oid.clone()),
        Ok(OrderResult::BatchAck { results }) => results.iter().find_map(|r| match r {
            Ok(a) if a.client_oid == client_oid => Some(a.exchange_oid.clone()),
            _ => None,
        }),
        _ => None,
    }
}

// Keep PlaceOne referenced for tests' import shape.
#[allow(dead_code)]
fn _anchor(_p: &PlaceOne, _a: &OrderAck) {}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::orders::{OrderAck, OrderKind, OrderResult, Side, Tif};

    fn place(client_oid: &str, symbol: &str) -> OrderRequest {
        OrderRequest {
            req_id: 1,
            exchange: ExchangeName::Kalshi,
            action: OrderAction::Place(PlaceOne {
                client_oid: client_oid.into(),
                symbol: symbol.into(),
                side: Side::Buy,
                kind: OrderKind::Limit,
                px: 0.5,
                qty: 1.0,
                tif: Tif::Gtc,
            }),
        }
    }

    fn ack(client_oid: &str, exchange_oid: &str) -> OrderResponse {
        OrderResponse {
            req_id: 1,
            result: Ok(OrderResult::Ack(OrderAck {
                client_oid: client_oid.into(),
                exchange_oid: exchange_oid.into(),
                status: OrderStatus::New,
                ts_ns: 0,
            })),
        }
    }

    #[test]
    fn record_then_lookup_idem_hits() {
        let r = OrderRegistry::with_capacity(8);
        let req = place("c-1", "X.YES");
        let resp = ack("c-1", "ex-1");
        r.record(&req, &resp);
        let got = r.lookup_idem("c-1").unwrap();
        assert_eq!(got.req_id, 1);
    }

    #[test]
    fn lookup_idem_miss() {
        let r = OrderRegistry::default();
        assert!(r.lookup_idem("nope").is_none());
    }

    #[test]
    fn expired_entry_evicted_on_lookup() {
        let r = OrderRegistry::with_capacity(8);
        let entry = OrderEntry {
            exchange: ExchangeName::Kalshi,
            symbol: Some("X".into()),
            response: ack("c-1", "ex-1"),
            exchange_oid: Some("ex-1".into()),
            status: OrderStatus::New,
            placed_at: Instant::now(),
            kind: IdemKind::Place,
            idem_expires_at: Instant::now() - Duration::from_millis(1),
        };
        r.seed("c-1".into(), entry);
        assert!(r.lookup_idem("c-1").is_none());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn live_set_excludes_terminals() {
        let r = OrderRegistry::with_capacity(8);
        r.record(&place("c-1", "X"), &ack("c-1", "ex-1"));
        r.record(&place("c-2", "Y"), &ack("c-2", "ex-2"));
        r.mark_terminal("c-2", OrderStatus::Canceled);
        let live = r.live_set(ExchangeName::Kalshi);
        assert!(live.contains("ex-1"));
        assert!(!live.contains("ex-2"));
    }

    #[test]
    fn reset_for_exchange_drops_only_matching() {
        let r = OrderRegistry::with_capacity(8);
        let mut req = place("c-poly", "T");
        req.exchange = ExchangeName::Polymarket;
        let mut resp = ack("c-poly", "ex-p");
        if let Ok(OrderResult::Ack(a)) = &mut resp.result {
            a.client_oid = "c-poly".into();
        }
        r.record(&req, &resp);
        r.record(&place("c-k", "X"), &ack("c-k", "ex-k"));
        r.reset_for_exchange(ExchangeName::Polymarket);
        assert!(r.lookup_idem("c-poly").is_none());
        assert!(r.lookup_idem("c-k").is_some());
    }

    #[test]
    fn capacity_evicts_fifo() {
        let r = OrderRegistry::with_capacity(2);
        r.record(&place("a", "X"), &ack("a", "ea"));
        r.record(&place("b", "X"), &ack("b", "eb"));
        r.record(&place("c", "X"), &ack("c", "ec"));
        // `a` was the oldest insert, should be evicted.
        assert!(r.lookup_idem("a").is_none());
        assert!(r.lookup_idem("b").is_some());
        assert!(r.lookup_idem("c").is_some());
    }
}
