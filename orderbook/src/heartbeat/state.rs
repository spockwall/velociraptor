use crate::heartbeat::HearthbeatConfig;
use crate::heartbeat::metrics::ConnectionMetrics;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct HearthbeatState {
    pub last_pong: Option<Instant>,
    pub last_ping_sent: Option<Instant>,
    pub consecutive_missed: u32,
    pub max_missed_pongs: u32,
    pub metrics: ConnectionMetrics,
    /// Track timeout boundaries to prevent duplicate increments
    pub last_timeout_boundary: Option<u32>,
}

/// Detailed connection statistics
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub uptime: Duration,
    pub success_rate: f64,
    pub avg_rtt: Option<Duration>,
    pub total_pings: u64,
    pub total_pongs: u64,
    pub current_streak: u32,
    pub max_missed_streak: u32,
    pub quality_score: f64,
    pub adaptive_timeout: Duration,
}

#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy {
        rtt: Option<Duration>,
        quality_score: f64,
    },
    Warning {
        elapsed: Duration,
        missed_count: u32,
        quality_score: f64,
    },
    Critical {
        elapsed: Option<Duration>,
        missed_count: u32,
    },
}

impl ConnectionStats {
    /// Format stats for logging
    pub fn format_summary(&self) -> String {
        format!(
            "Uptime: {:?}, Success: {:.1}%, RTT: {:?}, Quality: {:.2}, Streak: {}",
            self.uptime,
            self.success_rate * 100.0,
            self.avg_rtt,
            self.quality_score,
            self.current_streak
        )
    }
}

impl HearthbeatState {
    pub fn new(max_missed_pongs: u32) -> Self {
        Self {
            last_pong: Some(Instant::now()),
            last_ping_sent: None,
            consecutive_missed: 0,
            max_missed_pongs,
            metrics: ConnectionMetrics::new(),
            last_timeout_boundary: None,
        }
    }

    pub fn record_pong(&mut self) {
        let now = Instant::now();
        self.last_pong = Some(now);

        // Calculate RTT if we have a recent ping
        if let Some(ping_time) = self.last_ping_sent {
            let rtt = now.duration_since(ping_time);
            self.metrics.record_rtt(rtt);
        }

        self.metrics.pongs_received += 1;
        self.metrics.consecutive_pongs += 1;

        // Reset missed count and boundary tracking
        if self.consecutive_missed > 0 {
            self.metrics.max_missed_streak =
                self.metrics.max_missed_streak.max(self.consecutive_missed);
        }
        self.consecutive_missed = 0;
        self.last_timeout_boundary = None;
    }

    pub fn record_ping_sent(&mut self) {
        self.last_ping_sent = Some(Instant::now());
        self.metrics.pings_sent += 1;
        self.metrics.consecutive_pongs = 0;
    }

    pub fn reset_connection(&mut self) {
        self.last_pong = Some(Instant::now());
        self.last_ping_sent = None;
        self.consecutive_missed = 0;
        self.last_timeout_boundary = None;
        self.metrics = ConnectionMetrics::new();
    }

    /// Get adaptive timeout based on current RTT
    fn get_adaptive_timeout(&self, base_timeout: Duration, config: &HearthbeatConfig) -> Duration {
        if !config.adaptive_timeout {
            return base_timeout;
        }

        if let Some(avg_rtt) = self.metrics.avg_rtt {
            let adaptive = Duration::from_millis(
                (avg_rtt.as_millis() as f64 * config.adaptive_multiplier) as u64,
            );
            adaptive.clamp(config.min_adaptive_timeout, config.max_adaptive_timeout)
        } else {
            base_timeout
        }
    }

    pub fn check_health(
        &mut self,
        base_timeout: Duration,
        config: &HearthbeatConfig,
    ) -> HealthStatus {
        let adaptive_timeout = self.get_adaptive_timeout(base_timeout, config);

        match self.last_pong {
            Some(last) => {
                let elapsed = last.elapsed();
                let expected_timeouts = (elapsed.as_millis() / adaptive_timeout.as_millis()) as u32;

                // Only increment missed count when crossing new timeout boundaries
                if expected_timeouts > 0 && self.last_timeout_boundary != Some(expected_timeouts) {
                    self.consecutive_missed = expected_timeouts;
                    self.last_timeout_boundary = Some(expected_timeouts);
                }

                let quality = self.metrics.quality_score();

                if elapsed <= adaptive_timeout {
                    HealthStatus::Healthy {
                        rtt: self.metrics.avg_rtt,
                        quality_score: quality,
                    }
                } else if self.consecutive_missed < self.max_missed_pongs {
                    HealthStatus::Warning {
                        elapsed,
                        missed_count: self.consecutive_missed,
                        quality_score: quality,
                    }
                } else {
                    HealthStatus::Critical {
                        elapsed: Some(elapsed),
                        missed_count: self.consecutive_missed,
                    }
                }
            }
            None => HealthStatus::Critical {
                elapsed: None,
                missed_count: 0,
            },
        }
    }

    /// Get connection statistics
    pub fn get_stats(&self, base_timeout: Duration, config: &HearthbeatConfig) -> ConnectionStats {
        ConnectionStats {
            uptime: self.metrics.uptime(),
            success_rate: self.metrics.success_rate(),
            avg_rtt: self.metrics.avg_rtt,
            total_pings: self.metrics.pings_sent,
            total_pongs: self.metrics.pongs_received,
            current_streak: self.metrics.consecutive_pongs,
            max_missed_streak: self.metrics.max_missed_streak,
            quality_score: self.metrics.quality_score(),
            adaptive_timeout: self.get_adaptive_timeout(base_timeout, config),
        }
    }

    pub fn get_metrics(&self) -> &ConnectionMetrics {
        &self.metrics
    }

    /// Check if connection should be considered stable (5min + 95% success + 10+ streak)
    pub fn is_stable_connection(&self) -> bool {
        let metrics = &self.metrics;
        metrics.uptime() > Duration::from_secs(300)
            && metrics.success_rate() > 0.95
            && metrics.consecutive_pongs > 10
    }
}
