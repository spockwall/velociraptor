//! The generic Redis-write hook used by the **main** `StreamEngine`.
//!
//! This hook keys Redis writes by `snap.symbol`, which is the right thing
//! ONLY for static exchanges where the symbol IS the stable identifier
//! (`btcusdt`, `BTC-USDT`, `BTC`, …). For rolling markets (Polymarket Up/Down,
//! Kalshi 15-min) the per-window engine MUST NOT use this attacher — its
//! `snap.symbol` is the per-window `asset_id` / `ticker`, which would create
//! one stale Redis entry per rolled-off window. Rolling markets write Redis
//! from their own per-window forward hooks (see [`super::rolling`]) keyed by
//! the stable `base_slug` / `series` instead.

use crate::topics::bba::BbaPayload;
use libs::protocol::{LastTradePrice, OrderbookSnapshot, UserEvent};
use libs::redis_client::{
    keys::{Events, RedisKey},
    RedisHandle,
};
use orderbook::StreamEngine;
use tracing::info;

/// Register engine hooks that write market and account state to Redis.
///
/// - Latest snapshot: `SET ob:{exchange}:{symbol}` (overwritten each tick)
/// - Latest BBA:      `SET bba:{exchange}:{symbol}` (overwritten each tick)
/// - Recent snapshots: `LPUSH snapshots:{exchange}:{symbol}`, capped at `snapshot_cap`
/// - Recent trades:    `LPUSH trades:{exchange}:{symbol}`, capped at `trade_cap`
/// - Event lists:      `LPUSH events:fills`, `LPUSH events:orders`, capped at `event_list_cap`
///
/// Hooks fire synchronously — all Redis I/O is dispatched via `tokio::spawn`.
pub fn attach_redis(
    engine: &mut StreamEngine,
    handle: RedisHandle,
    snapshot_cap: usize,
    trade_cap: usize,
) {
    // Orderbook snapshot hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<OrderbookSnapshot, _>(move |snap| {
            let exchange = snap.exchange.to_str().to_owned();
            let symbol = snap.symbol.clone();

            // Latest full snapshot (overwrite)
            if let Ok(ob_bytes) = rmp_serde::to_vec_named(snap) {
                let h2 = h.clone();
                let ex2 = exchange.clone();
                let sym2 = symbol.clone();
                let ob2 = ob_bytes.clone();
                tokio::spawn(async move { h2.set_orderbook(&ex2, &sym2, &ob2).await });

                // Recent snapshot list (capped)
                let h3 = h.clone();
                let ex3 = exchange.clone();
                let sym3 = symbol.clone();
                tokio::spawn(async move {
                    h3.lpush_capped(&RedisKey::snapshots(&ex3, &sym3), &ob_bytes, snapshot_cap)
                        .await;
                });
            }

            // Latest BBA (overwrite) — reuse BbaPayload from topics::bba
            if let Ok(bba_bytes) = rmp_serde::to_vec_named(&BbaPayload::from(snap)) {
                let h2 = h.clone();
                let ex2 = exchange.clone();
                let sym2 = symbol.clone();
                tokio::spawn(async move { h2.set_bba(&ex2, &sym2, &bba_bytes).await });
            }
        });
    }

    // Last-trade hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<LastTradePrice, _>(move |trade| {
            let Ok(bytes) = rmp_serde::to_vec_named(trade) else {
                return;
            };
            let h2 = h.clone();
            let exchange = trade.exchange.to_str().to_owned();
            let symbol = trade.symbol.clone();
            tokio::spawn(async move {
                h2.lpush_capped(&RedisKey::trades(&exchange, &symbol), &bytes, trade_cap)
                    .await;
            });
        });
    }

    // User events hook
    {
        let h = handle.clone();
        engine.hooks_mut().on::<UserEvent, _>(move |ev| {
            let Ok(payload) = rmp_serde::to_vec_named(ev) else {
                return;
            };
            let h2 = h.clone();
            let ev = ev.clone();
            tokio::spawn(async move {
                match &ev {
                    UserEvent::Fill { .. } => {
                        h2.lpush_capped(Events::FILLS, &payload, h2.event_list_cap)
                            .await;
                    }
                    UserEvent::OrderUpdate { .. } => {
                        h2.lpush_capped(Events::ORDERS, &payload, h2.event_list_cap)
                            .await;
                    }
                }
            });
        });
    }

    info!("Redis integration enabled (snapshot_cap={snapshot_cap}, trade_cap={trade_cap})");
}
