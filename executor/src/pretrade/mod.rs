//! Pre-trade risk gate — the only subsystem that can reject an order on
//! risk grounds. Self-contained: rules + runner + state + context.
//!
//! `PretradeRunner` walks the configured rules for each `Place` leg; rules
//! whose backing `PretradeLimits` field is `None` are no-ops. Future risk
//! categories (position, exposure) will be **sibling modules**
//! (`executor::position`, `executor::exposure`) with their own runner +
//! state — there is no shared umbrella trait here by design.

mod context;
mod engine;
mod rules;
mod state;

pub use engine::PretradeEngine;
pub use state::PretradeState;

// `PretradeRule` + `PretradeContext` are crate-internal: the 5 default
// rules are plenty for now, and an external rule would also need
// `PretradeContext` which is intentionally not exposed. A future
// extension point would re-export both.
