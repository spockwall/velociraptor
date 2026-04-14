//! Executor — ZMQ REP gateway that receives `OrderRequest`s from the Python
//! trading engine and forwards them to per-exchange REST clients
//! (Polymarket, Kalshi).
//!
//! See the Phase 3 section of the implementation plan for the full trait
//! design (`RestOrderClient`), heartbeat/dead-man-switch semantics, and
//! per-exchange endpoint mapping. This crate currently contains only the
//! top-level stub so the workspace compiles; real logic lands in Phase 3.
