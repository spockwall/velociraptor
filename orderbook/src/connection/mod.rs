pub mod client;
pub mod configs;
pub mod constants;
pub mod traits;

pub use configs::ClientConfig;
pub use constants::PAUSE_DELAY;
pub use traits::{AuthHeader, BasicClientMsgTrait, ClientTrait, MsgParserTrait};

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BaseClientMessage {
    Ping,
    Pong,
    Error(String),
    Connected,
    Disconnected,
}

pub struct SystemControl {
    pub pause: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
}

impl SystemControl {
    pub fn new() -> Self {
        Self {
            pause: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn pause(&self) {
        self.pause.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.pause
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Clone for SystemControl {
    fn clone(&self) -> Self {
        Self {
            pause: Arc::clone(&self.pause),
            shutdown: Arc::clone(&self.shutdown),
        }
    }
}
