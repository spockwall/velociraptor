//! ROUTER frame parsing + reply construction.
//!
//! Consumes raw `Multipart` frames from the ROUTER socket, mutates the
//! subscription registry, and returns the reply frames to send back.

use crate::control::registry::{Registry, apply_request};
use crate::protocol::{Ack, SubscriptionRequest};
use crate::trading::events::ChannelRequest;
use crate::types::Action;
use tmq::Multipart;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Parse router frames and return reply frames.
pub fn handle_control(
    frames: Multipart,
    registry: &mut Registry,
    channel_tx: &mpsc::UnboundedSender<ChannelRequest>,
) -> Option<Vec<Vec<u8>>> {
    let frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.to_vec()).collect();
    let has_delimiter = frames.len() >= 3;
    let (client_id, payload) = if has_delimiter {
        (frames[0].clone(), &frames[2])
    } else if frames.len() == 2 {
        (frames[0].clone(), &frames[1])
    } else {
        warn!("ZMQ ROUTER: malformed message ({} frames)", frames.len());
        return None;
    };

    let make_reply = |json: Vec<u8>| -> Vec<Vec<u8>> {
        if has_delimiter {
            vec![client_id.clone(), vec![], json]
        } else {
            vec![client_id.clone(), json]
        }
    };

    let req = match serde_json::from_slice::<SubscriptionRequest>(payload) {
        Err(e) => {
            let ack = Ack::error(format!("parse error: {e}"));
            let json = serde_json::to_vec(&ack).ok()?;
            return Some(make_reply(json));
        }
        Ok(r) => r,
    };

    let ack = match req.action {
        Action::Subscribe => {
            apply_request(&req, client_id.clone(), registry);
            Ack::ok(&req)
        }
        Action::Unsubscribe => {
            apply_request(&req, client_id.clone(), registry);
            Ack::ok_unsub(&req)
        }
        Action::AddChannel => {
            info!(
                "Client {:?} requesting new channel {}:{}",
                client_id, req.exchange, req.symbol
            );
            let cr = ChannelRequest {
                client_id: client_id.clone(),
                exchange: req.exchange.clone(),
                symbol: req.symbol.clone(),
            };
            if let Err(e) = channel_tx.send(cr) {
                error!("Failed to forward add_channel request: {e}");
                Ack::error("internal error: channel request dropped")
            } else {
                Ack::ok_add_channel(&req.exchange, &req.symbol)
            }
        }
    };

    let json = serde_json::to_vec(&ack).ok()?;
    Some(make_reply(json))
}
