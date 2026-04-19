//! ROUTER socket wrapper.

use futures_util::{SinkExt, StreamExt};
use tmq::{router, router::Router, Context, Multipart};
use tracing::{info, warn};

pub struct RouterSocket {
    inner: Router,
}

/// The parts extracted from a raw ROUTER multipart message.
pub struct RouterFrame {
    pub client_id: Vec<u8>,
    pub payload: Vec<u8>,
    /// Whether the message included a ZMQ delimiter frame (empty frame between
    /// identity and payload). Used to mirror the same structure in the reply.
    pub has_delimiter: bool,
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

/// Parse a raw ROUTER `Multipart` into a `RouterFrame`.
///
/// Returns `None` and logs a warning if the frame count is unexpected.
pub fn parse_router_frames(frames: Multipart) -> Option<RouterFrame> {
    let frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.to_vec()).collect();
    match frames.len() {
        n if n >= 3 => Some(RouterFrame {
            client_id: frames[0].clone(),
            payload: frames[2].clone(),
            has_delimiter: true,
        }),
        2 => Some(RouterFrame {
            client_id: frames[0].clone(),
            payload: frames[1].clone(),
            has_delimiter: false,
        }),
        n => {
            warn!("ZMQ ROUTER: malformed message ({n} frames)");
            None
        }
    }
}
