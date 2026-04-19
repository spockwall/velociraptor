//! Topic trait and all concrete topic implementations.
//!
//! Every ZMQ PUB channel is modelled as a type that implements [`Topic`].
//! Adding a new channel means adding one new file here — nothing else changes.
//!
//! # Layout
//! - [`Topic`] — the encoding contract all channels share.
//! - [`market`] — orderbook snapshot and BBA channels.
//! - [`user`]   — private user-event channel.

pub mod bba;
pub mod snapshot;
pub mod user;

/// Encoding contract for a ZMQ PUB topic.
///
/// Each implementor knows its own topic string and how to serialise itself
/// into msgpack bytes. The server calls `encode` and forwards the result
/// directly to `PubSocket::send`.
pub trait Topic {
    /// The ZMQ subscription prefix for this message (e.g. `"binance:btcusdt"`).
    fn topic(&self) -> String;

    /// Encode the payload as msgpack bytes.
    ///
    /// Returns `None` if serialisation fails (the error is logged by the
    /// implementor so the server can silently skip the frame).
    fn encode(&self) -> Option<Vec<u8>>;

    /// Convenience: return `(topic, bytes)` or `None`.
    fn frame(&self) -> Option<(String, Vec<u8>)> {
        Some((self.topic(), self.encode()?))
    }
}

/// Decode raw msgpack bytes for a topic whose payload type implements
/// `serde::Deserialize`.
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, rmp_serde::decode::Error> {
    rmp_serde::from_slice(bytes)
}
