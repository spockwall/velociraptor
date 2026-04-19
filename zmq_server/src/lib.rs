//! Transport layer: binds market-data PUB, control ROUTER, and user-event PUB
//! ZMQ sockets, and forwards events from a `StreamEventSource` implementor
//! (supplied by the `orderbook` crate's `StreamSystem`).

pub mod control;
pub mod frame;
pub mod protocol;
pub mod server;
pub mod setup;
pub mod socket;
pub mod types;

pub use orderbook::{
    OrderbookSnapshot, StreamEngine, StreamEngineBus, StreamEngineHandle, StreamEvent,
    StreamEventSource, StreamSystem, StreamSystemConfig,
};
pub use server::ZmqServer;
pub use types::{Action, SubscriptionKey, SubscriptionType};
