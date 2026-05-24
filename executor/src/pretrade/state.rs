//! Pre-trade risk state: the hot-reloadable config snapshot plus the
//! per-(exchange, symbol) bookkeeping the rules read from.
//!
//! Lives here (not in `control/`) because it's risk-domain data, not an
//! operational signal. The control thread can still trigger a swap via the
//! `ControlCallbacks::reload_risk` hook on `Executor`.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use libs::configs::RiskConfig;
use libs::protocol::ExchangeName;

/// Rolling window for the per-(exchange,symbol) placement rate limit.
const RATE_WINDOW: Duration = Duration::from_secs(60);

type SymKey = (ExchangeName, String);

pub struct PretradeState {
    /// Active risk limits. Swapped wholesale by the config-reload watcher;
    /// read (cloned `Arc`) on the order hot path.
    config: RwLock<Arc<RiskConfig>>,
    /// Best-effort open-order count per (exchange, symbol). Incremented on a
    /// successful place, decremented on a successful cancel/cancel-market.
    /// May over-count (fills/expiries don't pass through the gateway) —
    /// fail-safe for a *max* limit. Reset for a symbol on cancel-all.
    open_orders: DashMap<SymKey, u32>,
    /// Sliding-window placement timestamps per (exchange, symbol) for the
    /// rate-limit rule. Trimmed to the last `RATE_WINDOW` on each query.
    place_times: DashMap<SymKey, VecDeque<Instant>>,
}

impl Default for PretradeState {
    fn default() -> Self {
        Self {
            config: RwLock::new(Arc::new(RiskConfig::default())),
            open_orders: DashMap::new(),
            place_times: DashMap::new(),
        }
    }
}

impl PretradeState {
    fn key(exchange: ExchangeName, symbol: &str) -> SymKey {
        (exchange, symbol.to_string())
    }

    /// Cheap snapshot of the active risk config (clones the `Arc`).
    pub fn snapshot(&self) -> Arc<RiskConfig> {
        self.config.read().expect("risk lock poisoned").clone()
    }

    /// Replace the active risk config (called by the reload pathway).
    pub fn swap(&self, cfg: RiskConfig) {
        *self.config.write().expect("risk lock poisoned") = Arc::new(cfg);
    }

    pub fn open_count(&self, exchange: ExchangeName, symbol: &str) -> u32 {
        self.open_orders
            .get(&Self::key(exchange, symbol))
            .map(|v| *v)
            .unwrap_or(0)
    }

    pub fn incr_open(&self, exchange: ExchangeName, symbol: &str) {
        *self
            .open_orders
            .entry(Self::key(exchange, symbol))
            .or_insert(0) += 1;
    }

    pub fn decr_open(&self, exchange: ExchangeName, symbol: &str) {
        if let Some(mut v) = self.open_orders.get_mut(&Self::key(exchange, symbol)) {
            *v = v.saturating_sub(1);
        }
    }

    /// Reset every symbol's open-order count for `exchange` to 0 (cancel-all).
    pub fn reset_open_for(&self, exchange: ExchangeName) {
        for mut e in self.open_orders.iter_mut() {
            if e.key().0 == exchange {
                *e.value_mut() = 0;
            }
        }
    }

    /// Record a placement now and return how many fall within the last
    /// `RATE_WINDOW` for (exchange, symbol) (including this one).
    pub fn record_and_count_rate(&self, exchange: ExchangeName, symbol: &str) -> u32 {
        let now = Instant::now();
        let mut dq = self
            .place_times
            .entry(Self::key(exchange, symbol))
            .or_default();
        dq.push_back(now);
        while let Some(front) = dq.front() {
            if now.duration_since(*front) > RATE_WINDOW {
                dq.pop_front();
            } else {
                break;
            }
        }
        dq.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_count_default_zero() {
        let s = PretradeState::default();
        assert_eq!(s.open_count(ExchangeName::Kalshi, "X"), 0);
    }

    #[test]
    fn incr_decr_round_trip() {
        let s = PretradeState::default();
        s.incr_open(ExchangeName::Kalshi, "X");
        s.incr_open(ExchangeName::Kalshi, "X");
        assert_eq!(s.open_count(ExchangeName::Kalshi, "X"), 2);
        s.decr_open(ExchangeName::Kalshi, "X");
        assert_eq!(s.open_count(ExchangeName::Kalshi, "X"), 1);
    }

    #[test]
    fn reset_open_for_clears_one_exchange() {
        let s = PretradeState::default();
        s.incr_open(ExchangeName::Kalshi, "X");
        s.incr_open(ExchangeName::Polymarket, "Y");
        s.reset_open_for(ExchangeName::Kalshi);
        assert_eq!(s.open_count(ExchangeName::Kalshi, "X"), 0);
        assert_eq!(s.open_count(ExchangeName::Polymarket, "Y"), 1);
    }

    #[test]
    fn rate_records_and_counts() {
        let s = PretradeState::default();
        assert_eq!(s.record_and_count_rate(ExchangeName::Kalshi, "X"), 1);
        assert_eq!(s.record_and_count_rate(ExchangeName::Kalshi, "X"), 2);
    }
}
