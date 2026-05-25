//! Capability traits implemented by [`crate::executor::Executor`].
//!
//! The order-handling code (`Executor::handle_one`) calls `self.foo(...)`
//! and only requires that `Self: Audit + Idempotent + Gated + Dispatch + ...`
//! — so the per-request logic is testable against any composition of
//! impls. Today there is exactly one composition: `Executor`.
//!
//! Each trait covers one operational concern. The split mirrors the
//! state-maintenance steps inside `handle_one`:
//!
//! - [`Audit`]      — append-only log of every request + response
//! - [`Idempotent`] — `client_oid` replay short-circuit
//! - [`Registry`]   — update the shared order registry post-dispatch
//! - [`Gated`]      — operational gate (kill-switch / pause / deadman)
//! - [`Dispatch`]   — actually send the order to the exchange REST client
//! - [`Risk`]       — pre-trade risk pipeline (the only thing that can
//!                    reject on risk grounds)
//! - [`Meter`]      — metrics counters
//!
//! Position / Exposure subsystems would extend `Risk` (or, if they
//! evolve independently, get their own traits) — `handle_one` does not
//! need to change.

use async_trait::async_trait;
use libs::protocol::orders::{OrderRequest, OrderResponse, OrderResult, OrderError};

#[async_trait]
pub trait Audit: Send + Sync {
    async fn audit_request(&self, req: &OrderRequest);
    async fn audit_response(
        &self,
        req_id: u64,
        result: &Result<OrderResult, OrderError>,
        critical: bool,
    );
}

pub trait Idempotent: Send + Sync {
    /// Returns a cached response if `req` is a retry of a still-tracked
    /// Place / Cancel.
    fn idem_lookup(&self, req: &OrderRequest) -> Option<OrderResponse>;
    /// Records `(req, response)` in the registry so a later retry hits.
    fn idem_stash(&self, req: &OrderRequest, response: &OrderResponse);
}

pub trait Registry: Send + Sync {
    /// Post-dispatch bookkeeping: bump open-order counts, mark entries
    /// terminal if the response says so, etc.
    fn update_registry(&self, req: &OrderRequest, result: &Result<OrderResult, OrderError>);
}

pub trait Gated: Send + Sync {
    /// `true` when the executor must reject this `req` for operational
    /// reasons (kill / pause / deadman / per-exchange kill). Cancel-class
    /// actions ignore this gate.
    fn is_blocked(&self, req: &OrderRequest) -> bool;
}

#[async_trait]
pub trait Dispatch: Send + Sync {
    /// Forward `req` to the configured REST client.
    async fn dispatch(&self, req: &OrderRequest) -> Result<OrderResult, OrderError>;
}

#[async_trait]
pub trait Risk: Send + Sync {
    /// Run the pre-trade pipeline. Returns `Some((rule, detail))` on the
    /// first rejection, `None` if the order passes (or risk is
    /// disabled / unconfigured for this scope).
    async fn run_pretrade(&self, req: &OrderRequest) -> Option<(&'static str, String)>;
}

pub trait Meter: Send + Sync {
    fn inc_risk_rejection(&self, rule: &str);
    fn set_open_orders(&self, exchange: &str, symbol: &str, count: i64);
}
