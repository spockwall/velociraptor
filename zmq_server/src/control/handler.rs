//! ROUTER frame parsing + reply construction.
//!
//! Consumes raw `Multipart` frames from the ROUTER socket, mutates the
//! subscription registry, and returns the reply frames to send back.

use crate::control::registry::{handle_request, Registry};
use crate::protocol::{Ack, Action, SubscriptionRequest};
use crate::socket::router::RouterFrame;

/// Handle a parsed `RouterFrame`: decode the JSON request, update the registry,
/// and return the reply frames.
pub fn handle_control(frame: RouterFrame, registry: &mut Registry) -> Option<Vec<Vec<u8>>> {
    let RouterFrame {
        client_id,
        payload,
        has_delimiter,
    } = frame;

    let make_reply = |json: Vec<u8>| -> Vec<Vec<u8>> {
        if has_delimiter {
            vec![client_id.clone(), vec![], json]
        } else {
            vec![client_id.clone(), json]
        }
    };

    let req = match serde_json::from_slice::<SubscriptionRequest>(&payload) {
        Err(e) => {
            let ack = Ack::error(format!("parse error: {e}"));
            let json = serde_json::to_vec(&ack).ok()?;
            return Some(make_reply(json));
        }
        Ok(r) => r,
    };

    let ack = match req.action {
        Action::Subscribe => {
            handle_request(&req, client_id.clone(), registry);
            Ack::ok(&req)
        }
        Action::Unsubscribe => {
            handle_request(&req, client_id.clone(), registry);
            Ack::ok_unsub(&req)
        }
    };

    let json = serde_json::to_vec(&ack).ok()?;
    Some(make_reply(json))
}
