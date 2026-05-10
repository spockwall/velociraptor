//! Idempotency-aware retry + per-exchange circuit breaker.
//!
//! Rules:
//! - `is_connect()` → retry with exp backoff (3 attempts, 100ms × 2ⁿ + jitter).
//! - `is_timeout()` on `Place`/`Update` → **do not retry** (effect unknown).
//! - `is_timeout()` on idempotent ops (`Cancel`/`GetOrder`/`Heartbeat`) → retry.
//! - HTTP 5xx → retry up to 2 times for GET/DELETE; surface for POST `Place`.
//! - HTTP 4xx → never retry; surface as typed error.
//!
//! Circuit breaker state machine per exchange:
//!   Closed → (5 consecutive 5xx/connect errors) → Open (10s, fail-fast)
//!   Open → (timer expires) → HalfOpen (allow 1 probe)
//!   HalfOpen → success → Closed; failure → Open with doubled timeout.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use libs::protocol::orders::OrderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Place / Update — non-idempotent on the wire. Timeouts must NOT retry.
    Mutating,
    /// Cancel / GetOrder / Heartbeat / GetOrders — idempotent on the wire.
    Idempotent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Retry,
    Surface,
}

pub fn classify(err: &OrderError, action: Action, attempt: u32) -> Decision {
    if attempt >= 3 {
        return Decision::Surface;
    }
    match err {
        OrderError::Network { message } if message.starts_with("connect:") => Decision::Retry,
        OrderError::Timeout => match action {
            Action::Mutating => Decision::Surface,
            Action::Idempotent => Decision::Retry,
        },
        OrderError::ExchangeRejected { code, .. } => {
            // 5xx: retry idempotent ops up to 2 attempts; surface for mutating.
            if let Some(c) = code.as_deref() {
                if c.starts_with('5') && action == Action::Idempotent && attempt < 2 {
                    return Decision::Retry;
                }
            }
            Decision::Surface
        }
        _ => Decision::Surface,
    }
}

/// Sleep duration for `attempt` (0-indexed): 100ms × 2^attempt + small jitter.
pub fn backoff(attempt: u32) -> Duration {
    let base = 100u64.saturating_mul(1 << attempt.min(6));
    // Pseudo-jitter via attempt (deterministic, fine for backoff).
    Duration::from_millis(base + (attempt as u64 * 17) % 50)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    inner: Mutex<BreakerInner>,
}

struct BreakerInner {
    state: BreakerState,
    consecutive_failures: u32,
    open_until: Option<Instant>,
    current_open_secs: u64,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BreakerInner {
                state: BreakerState::Closed,
                consecutive_failures: 0,
                open_until: None,
                current_open_secs: 10,
            }),
        }
    }

    /// Check whether a call is allowed right now. If the breaker is Open and
    /// the timer has expired, transitions to HalfOpen and allows one probe.
    pub fn allow(&self) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.state {
            BreakerState::Closed => true,
            BreakerState::Open => {
                if let Some(t) = g.open_until {
                    if Instant::now() >= t {
                        g.state = BreakerState::HalfOpen;
                        g.open_until = None;
                        return true;
                    }
                }
                false
            }
            BreakerState::HalfOpen => false,
        }
    }

    pub fn on_success(&self) {
        let mut g = self.inner.lock().unwrap();
        g.state = BreakerState::Closed;
        g.consecutive_failures = 0;
        g.open_until = None;
        g.current_open_secs = 10;
    }

    /// Record a circuit-eligible failure (5xx or connect). Other errors don't
    /// affect the breaker state.
    pub fn on_failure(&self) {
        let mut g = self.inner.lock().unwrap();
        match g.state {
            BreakerState::HalfOpen => {
                g.current_open_secs = (g.current_open_secs * 2).min(300);
                g.state = BreakerState::Open;
                g.open_until = Some(Instant::now() + Duration::from_secs(g.current_open_secs));
            }
            BreakerState::Closed => {
                g.consecutive_failures += 1;
                if g.consecutive_failures >= 5 {
                    g.state = BreakerState::Open;
                    g.open_until = Some(Instant::now() + Duration::from_secs(g.current_open_secs));
                }
            }
            BreakerState::Open => {}
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.inner.lock().unwrap().state, BreakerState::Open)
    }
}

/// Decide whether a given error should count against the breaker.
pub fn is_circuit_failure(err: &OrderError) -> bool {
    match err {
        OrderError::Network { message } if message.starts_with("connect:") => true,
        OrderError::ExchangeRejected { code, .. } => code
            .as_deref()
            .map(|c| c.starts_with('5'))
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_retry_on_timeout_for_mutating() {
        let err = OrderError::Timeout;
        assert_eq!(classify(&err, Action::Mutating, 0), Decision::Surface);
        assert_eq!(classify(&err, Action::Idempotent, 0), Decision::Retry);
    }

    #[test]
    fn retry_on_connect() {
        let err = OrderError::Network {
            message: "connect: refused".into(),
        };
        assert_eq!(classify(&err, Action::Mutating, 0), Decision::Retry);
        assert_eq!(classify(&err, Action::Mutating, 3), Decision::Surface);
    }

    #[test]
    fn retry_5xx_idempotent_only() {
        let err = OrderError::ExchangeRejected {
            code: Some("503".into()),
            message: "down".into(),
        };
        assert_eq!(classify(&err, Action::Idempotent, 0), Decision::Retry);
        assert_eq!(classify(&err, Action::Mutating, 0), Decision::Surface);
    }

    #[test]
    fn breaker_opens_after_5_failures() {
        let cb = CircuitBreaker::new();
        for _ in 0..4 {
            cb.on_failure();
        }
        assert!(cb.allow());
        cb.on_failure();
        assert!(!cb.allow());
        assert!(cb.is_open());
    }

    #[test]
    fn breaker_half_open_then_closed_on_success() {
        let cb = CircuitBreaker::new();
        // Force into Open by hand by mutating internals via 5 failures.
        for _ in 0..5 {
            cb.on_failure();
        }
        // Manually reset open_until to past so allow() transitions to HalfOpen.
        {
            let mut g = cb.inner.lock().unwrap();
            g.open_until = Some(Instant::now() - Duration::from_secs(1));
        }
        assert!(cb.allow());
        cb.on_success();
        assert!(cb.allow());
    }
}
