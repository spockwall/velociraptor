//! Unified PUB socket wrapper — used for both market-data and user-event buses.

use futures_util::SinkExt;
use tmq::{Context, publish, publish::Publish};
use tracing::info;

pub struct PubSocket {
    inner: Publish,
    label: &'static str,
}

impl PubSocket {
    /// Bind a PUB socket at `endpoint`. `label` is used only for logging.
    pub fn bind(ctx: &Context, endpoint: &str, label: &'static str) -> Result<Self, tmq::TmqError> {
        let inner = publish(ctx).bind(endpoint)?;
        info!("ZMQ {label} PUB bound to {endpoint}");
        Ok(Self { inner, label })
    }

    /// Send a `(topic, payload)` pair as a two-frame multipart message.
    pub async fn send(&mut self, topic: String, bytes: Vec<u8>) -> Result<(), tmq::TmqError> {
        self.inner.send(vec![topic.into_bytes(), bytes]).await
    }

    pub fn label(&self) -> &'static str {
        self.label
    }
}
