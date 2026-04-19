//! ROUTER frame parsing + reply construction.
//!
//! Consumes raw `Multipart` frames from the ROUTER socket, mutates the
//! subscription registry, and returns the reply frames to send back.

use crate::control::registry::{Registry, apply_request};
use crate::protocol::{Ack, SubscriptionRequest};
use crate::types::Action;
use tmq::Multipart;
use tracing::warn;

/// Parse router frames and return reply frames.
pub fn handle_control(frames: Multipart, registry: &mut Registry) -> Option<Vec<Vec<u8>>> {
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
    };

    let json = serde_json::to_vec(&ack).ok()?;
    Some(make_reply(json))
}
