//! System-wide constants.
//!
//! Configuration constants for WebSocket connections, timeouts, ZMQ sockets,
//! Redis keys, and performance targets.
//!
//! ZMQ socket paths are loaded from environment variables via `ZmqSettings`.

use std::time::Duration;

// ===========================================
// ZMQ IPC Socket Paths (defaults)
// ===========================================

/// Default ZMQ market data socket path
pub const MARKET_DATA_SOCKET: &str = "ipc:///tmp/trading/market_data.sock";

/// Default ZMQ control socket path
pub const CONTROL_SOCKET: &str = "ipc:///tmp/trading/control.sock";

/// Default ZMQ WebSocket orders socket path
pub const WS_ORDERS_SOCKET: &str = "ipc:///tmp/trading/ws_orders.sock";

/// Default ZMQ WebSocket status socket path
pub const WS_STATUS_SOCKET: &str = "ipc:///tmp/trading/ws_status.sock";

/// Default ZMQ engine metrics socket path
pub const ENGINE_METRICS_SOCKET: &str = "ipc:///tmp/trading/engine_metrics.sock";

// ===========================================
// Redis Keys
// ===========================================

/// Redis configuration key
pub const REDIS_CONFIG_KEY: &str = "config:trading";

/// Redis configuration version key
pub const REDIS_CONFIG_VERSION_KEY: &str = "config:version";

/// Redis configuration updates channel
pub const REDIS_CONFIG_UPDATES_CHANNEL: &str = "config:updates";

// ===========================================
// Performance Targets
// ===========================================

/// Target market data latency in microseconds
pub const TARGET_MARKET_DATA_LATENCY_US: u64 = 500;

/// Target order WebSocket latency in milliseconds
pub const TARGET_ORDER_WS_LATENCY_MS: u64 = 5;

/// Target order REST latency in milliseconds
pub const TARGET_ORDER_REST_LATENCY_MS: u64 = 50;

/// Target config reload time in milliseconds
pub const TARGET_CONFIG_RELOAD_MS: u64 = 10;

/// Target failover time in milliseconds
pub const TARGET_FAILOVER_MS: u64 = 100;

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
    // ZMQ Socket Constants
    // ===========================================

    #[test]
    fn test_zmq_socket_constants() {
        assert_eq!(MARKET_DATA_SOCKET, "ipc:///tmp/trading/market_data.sock");
        assert_eq!(CONTROL_SOCKET, "ipc:///tmp/trading/control.sock");
        assert_eq!(WS_ORDERS_SOCKET, "ipc:///tmp/trading/ws_orders.sock");
        assert_eq!(WS_STATUS_SOCKET, "ipc:///tmp/trading/ws_status.sock");
        assert_eq!(
            ENGINE_METRICS_SOCKET,
            "ipc:///tmp/trading/engine_metrics.sock"
        );
    }

    #[test]
    fn test_zmq_sockets_are_ipc() {
        assert!(MARKET_DATA_SOCKET.starts_with("ipc://"));
        assert!(CONTROL_SOCKET.starts_with("ipc://"));
        assert!(WS_ORDERS_SOCKET.starts_with("ipc://"));
        assert!(WS_STATUS_SOCKET.starts_with("ipc://"));
        assert!(ENGINE_METRICS_SOCKET.starts_with("ipc://"));
    }

    #[test]
    fn test_zmq_sockets_under_trading_dir() {
        assert!(MARKET_DATA_SOCKET.contains("/tmp/trading/"));
        assert!(CONTROL_SOCKET.contains("/tmp/trading/"));
        assert!(WS_ORDERS_SOCKET.contains("/tmp/trading/"));
        assert!(WS_STATUS_SOCKET.contains("/tmp/trading/"));
        assert!(ENGINE_METRICS_SOCKET.contains("/tmp/trading/"));
    }

    // ===========================================
    // Redis Keys
    // ===========================================

    #[test]
    fn test_redis_keys() {
        assert_eq!(REDIS_CONFIG_KEY, "config:trading");
        assert_eq!(REDIS_CONFIG_VERSION_KEY, "config:version");
        assert_eq!(REDIS_CONFIG_UPDATES_CHANNEL, "config:updates");
    }

    // ===========================================
    // Performance Targets
    // ===========================================

    #[test]
    fn test_performance_targets() {
        assert_eq!(TARGET_MARKET_DATA_LATENCY_US, 500);
        assert_eq!(TARGET_ORDER_WS_LATENCY_MS, 5);
        assert_eq!(TARGET_ORDER_REST_LATENCY_MS, 50);
        assert_eq!(TARGET_CONFIG_RELOAD_MS, 10);
        assert_eq!(TARGET_FAILOVER_MS, 100);
    }

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
