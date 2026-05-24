//! Pre-trade risk gate.
//!
//! A small, extensible rule engine evaluated for every `Place` (and every
//! leg of a `PlaceBatch`) before it reaches the exchange REST client. Rules
//! are trait objects, so adding a new check is: implement [`PretradeRule`], push
//! it into [`PretradeEngine::default`], and add the backing field to
//! `libs::configs::PretradeLimits`. The gateway call site never changes.
//!
//! A rule whose backing limit field is `None` is a no-op (`Ok(())`). The
//! engine returns the **first** failing rule's `(name, detail)` so the
//! response and metrics label are deterministic.
//!
//! Submodules:
//!   - [`context`] — `PretradeContext` and the `PretradeRule` trait
//!   - [`rules`]   — concrete rule implementations (qty/notional/…)
//!   - [`engine`]  — `PretradeEngine` (rule registry + evaluation)

mod context;
mod engine;
mod rules;

pub use context::{PretradeContext, PretradeRule};
pub use engine::PretradeEngine;
