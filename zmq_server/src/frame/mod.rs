//! Pure frame / payload encoding. No sockets, no I/O.
//!
//! Each submodule turns an in-process event into the `(topic, bytes)` pair
//! that a ZMQ PUB socket expects. Keeping encoding decoupled from sockets
//! makes the logic easy to unit-test and lets the socket layer stay a thin
//! I/O wrapper.

pub mod market;
pub mod user;
