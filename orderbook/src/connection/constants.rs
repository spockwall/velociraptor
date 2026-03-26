//! System-wide constants.
//!
//! Configuration constants for WebSocket connections, timeouts, ZMQ sockets,
//! Redis keys, and performance targets.
//!
//! ZMQ socket paths are loaded from environment variables via `ZmqSettings`.

use std::time::Duration;

// ===========================================
// Timeouts
// ===========================================

/// WebSocket disconnect timeout in seconds
pub const WS_DISCONNECT_TIMEOUT_SEC: u64 = 5;

/// Order acknowledgment timeout in seconds
pub const ORDER_ACK_TIMEOUT_SEC: u64 = 2;

/// Shutdown timeout in seconds
pub const SHUTDOWN_TIMEOUT_SEC: u64 = 5;

// ===========================================
// Retry Configuration
// ===========================================

/// Maximum number of retries
pub const MAX_RETRIES: u32 = 3;

/// Initial backoff time in seconds
pub const INITIAL_BACKOFF_SEC: f64 = 0.1;

/// Wait time for active connection in seconds
pub const WAIT_FOR_ACTIVE_SEC: f64 = 0.5;

/// Pause delay in milliseconds via system control
pub const PAUSE_DELAY: u64 = 300;

// ===========================================
// Ping Configuration
// ===========================================

/// WebSocket ping interval in seconds
pub const PING_INTERVAL: u64 = 5;

/// WebSocket pong timeout in seconds
pub const PONG_TIMEOUT: u64 = 5;

/// WebSocket close timeout in seconds
pub const CLOSE_TIMEOUT: u64 = 5;

// ===========================================
// Duration Helpers
// ===========================================

/// Get ping interval as Duration
pub const fn ping_interval() -> Duration {
    Duration::from_secs(PING_INTERVAL)
}

/// Get pong timeout as Duration
pub const fn pong_timeout() -> Duration {
    Duration::from_secs(PONG_TIMEOUT)
}

/// Get close timeout as Duration
pub const fn close_timeout() -> Duration {
    Duration::from_secs(CLOSE_TIMEOUT)
}

/// Get WebSocket disconnect timeout as Duration
pub const fn ws_disconnect_timeout() -> Duration {
    Duration::from_secs(WS_DISCONNECT_TIMEOUT_SEC)
}

/// Get order acknowledgment timeout as Duration
pub const fn order_ack_timeout() -> Duration {
    Duration::from_secs(ORDER_ACK_TIMEOUT_SEC)
}

/// Get shutdown timeout as Duration
pub const fn shutdown_timeout() -> Duration {
    Duration::from_secs(SHUTDOWN_TIMEOUT_SEC)
}

/// Get initial backoff as Duration
pub fn initial_backoff() -> Duration {
    Duration::from_secs_f64(INITIAL_BACKOFF_SEC)
}

/// Get wait for active as Duration
pub fn wait_for_active() -> Duration {
    Duration::from_secs_f64(WAIT_FOR_ACTIVE_SEC)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===========================================
    // Timeout Constants
    // ===========================================

    #[test]
    fn test_timeout_constants() {
        assert_eq!(WS_DISCONNECT_TIMEOUT_SEC, 5);
        assert_eq!(ORDER_ACK_TIMEOUT_SEC, 2);
        assert_eq!(SHUTDOWN_TIMEOUT_SEC, 5);
    }

    // ===========================================
    // Retry Configuration
    // ===========================================

    #[test]
    fn test_retry_constants() {
        assert_eq!(MAX_RETRIES, 3);
        assert_eq!(INITIAL_BACKOFF_SEC, 0.1);
        assert_eq!(WAIT_FOR_ACTIVE_SEC, 0.5);
    }

    // ===========================================
    // Ping Configuration
    // ===========================================

    #[test]
    fn test_ping_constants() {
        assert_eq!(PING_INTERVAL, 5);
        assert_eq!(PONG_TIMEOUT, 5);
        assert_eq!(CLOSE_TIMEOUT, 5);
    }

    // ===========================================
    // Duration Helpers
    // ===========================================

    #[test]
    fn test_duration_helpers() {
        assert_eq!(ping_interval(), Duration::from_secs(5));
        assert_eq!(pong_timeout(), Duration::from_secs(5));
        assert_eq!(close_timeout(), Duration::from_secs(5));
        assert_eq!(ws_disconnect_timeout(), Duration::from_secs(5));
        assert_eq!(order_ack_timeout(), Duration::from_secs(2));
        assert_eq!(shutdown_timeout(), Duration::from_secs(5));
        assert_eq!(initial_backoff(), Duration::from_millis(100));
        assert_eq!(wait_for_active(), Duration::from_millis(500));
    }
}
