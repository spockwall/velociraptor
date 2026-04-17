//! Transport layer: binds market-data PUB, control ROUTER, and user-event PUB
//! ZMQ sockets, and forwards events from an `EngineEventSource` implementor.
//! Also hosts `TradingEngine`/`TradingSystem` — the demuxing engine that
//! produces the event stream this transport consumes.

pub mod control;
pub mod frame;
pub mod protocol;
pub mod server;
pub mod socket;
pub mod trading;
pub mod types;

pub use server::ZmqServer;
pub use trading::{
    ChannelRequest, EngineEvent, EngineEventSource, OrderbookRawUpdate, OrderbookSnapshotPayload,
    TradingEngine, TradingEngineBus, TradingEngineHandle, TradingSystem, TradingSystemConfig,
};
pub use types::{Action, SubscriptionKey, SubscriptionType};
