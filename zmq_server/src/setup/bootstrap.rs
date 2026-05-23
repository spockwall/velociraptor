//! `StreamSystem` construction, ZMQ-server attachment, and the recorder bridge.
//!
//! These three helpers form the spine of `orderbook_server::main`: build the
//! engine + system, wire the ZMQ PUB/ROUTER sockets to its event bus, and
//! optionally bridge that bus to the on-disk recorder.

use crate::ZmqServer;
use libs::constants::WS_STATUS_SOCKET;
use libs::protocol::ExchangeName;
use orderbook::connection::{ClientConfig, SystemControl};
use orderbook::{StreamEngine, StreamEvent, StreamEventSource, StreamSystem, StreamSystemConfig};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Build the main `StreamSystem` from `engine` + `system_cfg`.
///
/// If `has_static` is false a dummy no-op exchange is inserted so the engine
/// starts and the ZMQ bus is available for Polymarket/Kalshi windows — the
/// rolling-market window engines are separate `StreamSystem`s that forward
/// onto the main bus via `engine_bus()`, so the main system must be running
/// even when no static exchanges are configured.
pub fn build_system(
    engine: StreamEngine,
    mut system_cfg: StreamSystemConfig,
    ctrl: SystemControl,
    has_static: bool,
) -> anyhow::Result<StreamSystem> {
    if has_static {
        system_cfg.validate()?;
    } else {
        system_cfg.with_exchange(
            ClientConfig::new(ExchangeName::Binance).set_subscription_message(String::new()),
        );
    }
    Ok(StreamSystem::new(engine, system_cfg, ctrl)?)
}

/// Start the ZMQ server and attach its handle to `system`.
///
/// The server subscribes to `system.engine_bus()` and re-publishes incoming
/// [`StreamEvent`]s on the appropriate ZMQ topic. All rolling-market frames
/// arrive here via [`StreamEvent::RollingSnapshot`] / `RollingLastTradePrice`,
/// which carry the stable `base_slug` (Polymarket) or `series` (Kalshi) used
/// as the ZMQ topic — see `crate::server` for dispatch.
pub fn attach_zmq(system: &mut StreamSystem, server_pub: &str, server_router: &str) {
    let handle = ZmqServer::new(server_router, server_pub, WS_STATUS_SOCKET)
        .start(Arc::new(system.engine_bus()));
    system.attach_handle(handle);
    info!("ZMQ PUB    {}", server_pub);
    info!("ZMQ ROUTER {}", server_router);
}

/// Wire a `recorder::StorageWriter` to the running `StreamSystem`. Snapshots,
/// last-trades, and user events flow into daily CSV files under
/// `cfg.base_path`. The recorder runs as a separate broadcast subscriber, so
/// it cannot block the engine or other consumers.
///
/// Layout produced on disk:
///
///   {base}/{exchange}/{symbol}/{YYYY-MM-DD}.csv          — orderbook snapshots
///   {base}/{exchange}/{symbol}/{YYYY-MM-DD}-trades.csv   — last-trade events
///   {base}/events/{YYYY-MM-DD}.csv                       — user events
///
/// Each `UserEvent` row has a `type` column ∈ {`fill`, `order_update`}.
///
/// `cfg` is the executor-/server-side `recorder::StorageConfig`. Pass `None`
/// to disable; the function is a no-op in that case.
pub fn attach_recorder(system: &mut StreamSystem, cfg: Option<recorder::StorageConfig>) {
    let Some(cfg) = cfg else { return };

    // Bridge `StreamEvent` (engine bus) → `RecorderEvent` (recorder bus).
    // A small bounded broadcast is enough; the writer consumes via tokio mpsc
    // semantics under the hood, but uses broadcast::Receiver so we can fan
    // out further later without changing the writer API.
    let (rec_tx, rec_rx) = tokio::sync::broadcast::channel::<recorder::RecorderEvent>(1024);
    let mut engine_rx = system.engine_bus().subscribe();

    let bridge = tokio::spawn(async move {
        loop {
            match engine_rx.recv().await {
                Ok(StreamEvent::OrderbookSnapshot(snap)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Snapshot(snap));
                }
                Ok(StreamEvent::LastTradePrice(trade)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Trade(trade));
                }
                // Rolling-market events carry the same payload as the static
                // variants (`full_slug` is already stamped); archive them
                // identically so per-(exchange, asset_id) files keep filling.
                Ok(StreamEvent::RollingSnapshot { snap, .. }) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Snapshot(snap));
                }
                Ok(StreamEvent::RollingLastTradePrice { trade, .. }) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::Trade(trade));
                }
                Ok(StreamEvent::User(ev)) => {
                    let _ = rec_tx.send(recorder::RecorderEvent::UserEvent(ev));
                }
                Ok(StreamEvent::OrderbookRaw(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("recorder bridge lagged, skipped {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        debug!("recorder bridge exiting");
    });
    system.attach_handle(bridge);

    let base_path = cfg.base_path.display().to_string();
    let writer_handle = recorder::StorageWriter::new(cfg).start(rec_rx);
    system.attach_handle(writer_handle);
    info!("recorder: archive writer attached, base={base_path}");
}
