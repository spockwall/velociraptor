//! `client_oid`-keyed idempotency cache.
//!
//! Network timeouts on `Place` are the highest-risk failure mode — the engine
//! doesn't know if the order reached the exchange and may retry. The cache
//! returns the prior `OrderResponse` for the same `client_oid` if one exists
//! within the TTL. We also keep a short Cancel/Update entry for symmetry.
//!
//! Bounded LRU at 10k entries; TTLs:
//!   - Place / PlaceBatch: 60s
//!   - Cancel / Update    : 10s

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use libs::protocol::orders::OrderResponse;
use lru::LruCache;

const DEFAULT_CAP: usize = 10_000;
pub const PLACE_TTL: Duration = Duration::from_secs(60);
pub const CANCEL_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Place,
    Cancel,
}

struct Entry {
    response: OrderResponse,
    expires: Instant,
}

pub struct IdempotencyCache {
    inner: Mutex<LruCache<String, Entry>>,
}

impl Default for IdempotencyCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAP)
    }
}

impl IdempotencyCache {
    pub fn with_capacity(cap: usize) -> Self {
        let cap = NonZeroUsize::new(cap.max(1)).unwrap();
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Look up a cached response. Returns `None` for a miss or an expired entry
    /// (expired entries are evicted on access).
    pub fn get(&self, client_oid: &str) -> Option<OrderResponse> {
        let mut g = self.inner.lock().unwrap();
        if let Some(entry) = g.peek(client_oid) {
            if Instant::now() < entry.expires {
                let resp = entry.response.clone();
                // touch LRU
                let _ = g.get(client_oid);
                return Some(resp);
            }
            g.pop(client_oid);
        }
        None
    }

    pub fn insert(&self, client_oid: String, response: OrderResponse, kind: Kind) {
        let ttl = match kind {
            Kind::Place => PLACE_TTL,
            Kind::Cancel => CANCEL_TTL,
        };
        let entry = Entry {
            response,
            expires: Instant::now() + ttl,
        };
        self.inner.lock().unwrap().put(client_oid, entry);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::protocol::orders::{OrderError, OrderResponse};

    fn err_resp(req_id: u64) -> OrderResponse {
        OrderResponse {
            req_id,
            result: Err(OrderError::Timeout),
        }
    }

    #[test]
    fn hit_returns_cached_response() {
        let c = IdempotencyCache::with_capacity(8);
        c.insert("c1".into(), err_resp(1), Kind::Place);
        let got = c.get("c1").unwrap();
        assert_eq!(got.req_id, 1);
    }

    #[test]
    fn miss_returns_none() {
        let c = IdempotencyCache::with_capacity(8);
        assert!(c.get("nope").is_none());
    }

    #[test]
    fn expired_entry_evicted_on_access() {
        let c = IdempotencyCache::with_capacity(8);
        // Insert with manually-expired entry.
        let resp = err_resp(7);
        let entry = Entry {
            response: resp,
            expires: Instant::now() - Duration::from_millis(1),
        };
        c.inner.lock().unwrap().put("stale".into(), entry);
        assert!(c.get("stale").is_none());
        assert_eq!(c.len(), 0);
    }
}
