//! Subscription registry + ROUTER control handling.
//!
//! `registry` holds the authoritative per-client subscription state and
//! computes dispatch lists. `handler` parses incoming ROUTER frames,
//! applies them to the registry, and returns the reply frames.

pub mod handler;
pub mod registry;

pub use handler::handle_control;
pub use registry::{Registry, dispatch};
