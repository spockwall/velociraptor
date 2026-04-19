//! ZMQ socket binding + I/O wrappers.
//!
//! These are thin adapters over `tmq` that hide the `(topic, bytes)` →
//! multipart detail and handle bind errors uniformly. Encoding and routing
//! live in `crate::frame` and `crate::control` respectively.

pub mod publish;
pub mod router;

pub use publish::PubSocket;
pub use router::{parse_router_frames, RouterSocket};
