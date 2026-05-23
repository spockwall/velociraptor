//! Wiring helpers for `orderbook_server`.
//!
//! Each submodule owns one concern; this file is a thin re-export shell so
//! callers (the `orderbook_server` binary, integration tests) keep using
//! `zmq_server::setup::*` paths unchanged.
//!
//! Layout:
//!
//! - [`exchanges`] — static (always-on) exchange registration: Binance,
//!   Binance Spot, OKX, Hyperliquid. No window lifecycle.
//! - [`bootstrap`] — [`StreamSystem`](orderbook::StreamSystem) construction,
//!   ZMQ-server attachment, and the recorder bridge.
//! - [`redis_attach`] — the generic `attach_redis` hook used by the **main**
//!   engine for static exchanges (keys by `snap.symbol`, which IS the stable
//!   identifier for static exchanges). Rolling markets do NOT use this hook;
//!   see the per-window forward hooks in [`polymarket`] / [`kalshi`].
//! - [`polymarket`] — Polymarket rolling Up/Down windows + the account-wide
//!   user-event channel.
//! - [`kalshi`] — Kalshi rolling 15-min windows.
//! - [`rolling`] — crate-private helpers shared by the two rolling-market
//!   plumbing modules: stale-label eviction, deferred (boundary-aligned)
//!   label registration, the per-window forward-hook pair, and the
//!   shutdown-cleanup watchdog.

mod bootstrap;
mod exchanges;
mod kalshi;
mod polymarket;
mod redis_attach;
mod rolling;

pub use bootstrap::{attach_recorder, attach_zmq, build_system};
pub use exchanges::{add_binance, add_binance_spot, add_hyperliquid, add_okx};
pub use kalshi::spawn_kalshi_schedulers;
pub use polymarket::{spawn_polymarket_schedulers, spawn_polymarket_user_channel};
pub use redis_attach::attach_redis;
