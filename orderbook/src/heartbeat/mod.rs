pub mod manager;
pub mod metrics;
pub mod protocol;
pub mod state;

pub use manager::HearthbeatManager;
pub use protocol::HearthbeatProtocol;
pub use state::HealthStatus;
use std::time::Duration;

/// Simplified configuration for ping/pong behavior
#[derive(Debug, Clone)]
pub struct HearthbeatConfig {
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    pub health_check_interval: Duration,
    pub max_missed_pongs: u32,
    /// Enable adaptive timeout based on RTT (optional, helps with latency variations)
    pub adaptive_timeout: bool,
    /// Multiplier for adaptive timeout (RTT * multiplier)  
    pub adaptive_multiplier: f64,
    /// Minimum timeout even with good RTT
    pub min_adaptive_timeout: Duration,
    /// Maximum timeout even with bad RTT
    pub max_adaptive_timeout: Duration,
}

impl Default for HearthbeatConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(60),
            health_check_interval: Duration::from_secs(10),
            max_missed_pongs: 2,
            adaptive_timeout: false,  // Disabled by default for simplicity
            adaptive_multiplier: 8.0, // RTT * 8 for timeout
            min_adaptive_timeout: Duration::from_secs(10),
            max_adaptive_timeout: Duration::from_secs(90),
        }
    }
}
