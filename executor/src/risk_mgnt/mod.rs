//! Pre-trade risk gate.
//!
//! A small, extensible rule engine evaluated for every `Place` (and every
//! leg of a `PlaceBatch`) before it reaches the exchange REST client. Rules
//! are trait objects, so adding a new check is: implement [`RiskRule`], push
//! it into [`RiskEngine::default`], and add the backing field to
//! `libs::configs::PretradeLimits`. The gateway call site never changes.
//!
//! A rule whose backing limit field is `None` is a no-op (`Ok(())`). The
//! engine returns the **first** failing rule's `(name, detail)` so the
//! response and metrics label are deterministic.
//!
//! Submodules:
//!   - [`context`] — `RiskContext` and the `RiskRule` trait
//!   - [`rules`]   — concrete rule implementations (qty/notional/…)
//!   - [`engine`]  — `RiskEngine` (rule registry + evaluation)

mod context;
mod engine;
mod rules;

pub use context::{RiskContext, RiskRule};
pub use engine::RiskEngine;
