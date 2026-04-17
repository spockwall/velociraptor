//! ROUTER socket wrapper.

use futures_util::{SinkExt, StreamExt};
use tmq::{Context, Multipart, router, router::Router};
use tracing::info;

pub struct RouterSocket {
    inner: Router,
}

impl RouterSocket {
    pub fn bind(ctx: &Context, endpoint: &str) -> Result<Self, tmq::TmqError> {
        let inner = router(ctx).bind(endpoint)?;
        info!("ZMQ ROUTER bound to {endpoint}");
        Ok(Self { inner })
    }

    pub async fn recv(&mut self) -> Option<Result<Multipart, tmq::TmqError>> {
        self.inner.next().await
    }

    pub async fn send(&mut self, frames: Vec<Vec<u8>>) -> Result<(), tmq::TmqError> {
        self.inner.send(frames).await
    }
}
