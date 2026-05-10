//! Executor — ZMQ ROUTER gateway that receives `OrderRequest`s from the
//! trading engine and forwards them to a Polymarket REST client.
//!
//! Module layout:
//! - [`gateway`]   — ZMQ ROUTER, request decoding, dispatch, idempotency
//! - [`control`]   — kill switch and graceful shutdown plumbing
//! - [`rest`]      — per-exchange REST order clients (Polymarket today)
//! - [`ops`]       — operational concerns (audit log, metrics, reconcile, secrets)
//! - [`error`]     — crate-level error type

pub mod control;
pub mod error;
pub mod gateway;
pub mod ops;
pub mod rest;
