use crate::connection::SystemControl;
use crate::orderbook::Orderbook;
use crate::orderbook::hooks::HookRegistry;
use crate::orderbook::utils::log_orderbook_bba;
use crate::types::orderbook::StreamMessage;
use crate::types::snapshot_from;
use crate::types::stream::{StreamEvent, StreamEventSource};
use libs::protocol::OrderbookSnapshot;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

/// Default depth used when materializing `OrderbookSnapshot`s from the engine.

/// Self-contained event-processing engine.
///
/// Demuxes `StreamMessage`s pushed on its mpsc into event kinds and
/// broadcasts them on a single `StreamEvent` channel. Callers can also
/// register typed hooks via `hooks_mut().on::<T>(...)` — these fire
/// synchronously after broadcast, in registration order.
pub struct StreamEngine {
    orderbooks: Arc<dashmap::DashMap<String, Arc<Orderbook>>>,
    /// Inbound `StreamMessage`s from clients (handed to clients via `message_sender()`).
    message_tx: mpsc::UnboundedSender<StreamMessage>,
    message_rx: mpsc::UnboundedReceiver<StreamMessage>,
    /// Outbound `StreamEvent`s broadcast to all subscribers (transport, hooks runners, etc).
    event_broadcast_tx: broadcast::Sender<StreamEvent>,
    snapshot_depth: usize,
    hooks: HookRegistry,
}

/// Handle to the running `StreamEngine` task.
pub struct StreamEngineHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

/// Cloneable `StreamEventSource` — hand this to transport layers so they can
/// subscribe without holding a direct reference to the engine.
#[derive(Clone)]
pub struct StreamEngineBus {
    event_broadcast_tx: broadcast::Sender<StreamEvent>,
}

impl StreamEventSource for StreamEngineBus {
    fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.event_broadcast_tx.subscribe()
    }
}

impl StreamEngine {
    pub fn new(event_broadcast_capacity: usize, snapshot_depth: usize) -> Self {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let (event_broadcast_tx, _) = broadcast::channel(event_broadcast_capacity);
        Self {
            orderbooks: Arc::new(dashmap::DashMap::new()),
            message_tx,
            message_rx,
            event_broadcast_tx,
            snapshot_depth,
            hooks: HookRegistry::new(),
        }
    }

    /// Returns a sender that clients use to push `StreamMessage`s in.
    pub fn get_message_sender(&self) -> mpsc::UnboundedSender<StreamMessage> {
        self.message_tx.clone()
    }

    /// Subscribe directly to the event broadcast stream.
    pub fn subscribe_event(&self) -> broadcast::Receiver<StreamEvent> {
        self.event_broadcast_tx.subscribe()
    }

    /// Cheap cloneable handle that implements `StreamEventSource`.
    pub fn bus(&self) -> StreamEngineBus {
        StreamEngineBus {
            event_broadcast_tx: self.event_broadcast_tx.clone(),
        }
    }

    /// Returns a shared handle to the live orderbook map.
    pub fn get_orderbooks(&self) -> Arc<dashmap::DashMap<String, Arc<Orderbook>>> {
        self.orderbooks.clone()
    }

    /// Mutable access to the hook registry. Register hooks before `start()`:
    /// `engine.hooks_mut().on::<OrderbookSnapshot>(|s| { ... });`
    pub fn hooks_mut(&mut self) -> &mut HookRegistry {
        &mut self.hooks
    }

    /// Consume the engine, spawn the processing task, and return a handle.
    pub fn start(self, system_control: SystemControl) -> StreamEngineHandle {
        let orderbooks = self.orderbooks;
        let mut message_rx = self.message_rx;
        let event_broadcast_tx = self.event_broadcast_tx;
        let snapshot_depth = self.snapshot_depth;
        let hooks = self.hooks;

        let handle = tokio::spawn(async move {
            info!("Stream engine started");

            loop {
                if system_control.is_shutdown() {
                    break;
                }

                match message_rx.recv().await {
                    Some(StreamMessage::OrderbookUpdate(update)) => {
                        let key = format!("{}:{}", update.exchange, update.symbol);

                        // Raw update hooks + broadcast BEFORE apply.
                        hooks.fire(&update);
                        let _ = event_broadcast_tx.send(StreamEvent::OrderbookRaw(update.clone()));

                        // Get or create the orderbook entry.
                        let mut entry = orderbooks.entry(key.clone()).or_insert_with(|| {
                            info!("Creating new orderbook for {key}");
                            Arc::new(Orderbook::new(
                                update.symbol.clone(),
                                update.exchange.clone(),
                            ))
                        });

                        // Apply update — get a mutable Orderbook via Arc::make_mut.
                        Arc::make_mut(&mut entry).apply_update(update);

                        log_orderbook_bba(&key, &entry);

                        // Snapshot hooks + broadcast AFTER apply.
                        let snapshot: OrderbookSnapshot = snapshot_from(&entry, snapshot_depth);
                        hooks.fire(&snapshot);
                        let _ = event_broadcast_tx.send(StreamEvent::OrderbookSnapshot(snapshot));
                    }
                    Some(StreamMessage::UserEvent(ev)) => {
                        hooks.fire(&ev);
                        let _ = event_broadcast_tx.send(StreamEvent::User(ev));
                    }
                    Some(StreamMessage::LastTradePrice(trade)) => {
                        hooks.fire(&trade);
                        let _ = event_broadcast_tx.send(StreamEvent::LastTradePrice(trade));
                    }
                    Some(StreamMessage::Base(b)) => {
                        debug!(?b, "Base connection message");
                    }
                    None => {
                        warn!("Channel closed, stopping stream engine");
                        break;
                    }
                }
            }

            info!("Stream engine stopped");
        });

        StreamEngineHandle { handle }
    }
}
