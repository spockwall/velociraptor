use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Connection quality metrics
#[derive(Debug, Clone)]
pub struct ConnectionMetrics {
    /// Average round trip time for pings
    pub avg_rtt: Option<Duration>,
    /// Recent RTT samples (last 10)
    rtt_samples: VecDeque<Duration>,
    /// Total pings sent
    pub pings_sent: u64,
    /// Total pongs received
    pub pongs_received: u64,
    /// Connection uptime since last reconnect
    pub connection_start: Instant,
    /// Number of consecutive successful pongs
    pub consecutive_pongs: u32,
    /// Maximum consecutive missed pongs seen
    pub max_missed_streak: u32,
}

impl ConnectionMetrics {
    pub fn new() -> Self {
        Self {
            avg_rtt: None,
            rtt_samples: VecDeque::with_capacity(10),
            pings_sent: 0,
            pongs_received: 0,
            connection_start: Instant::now(),
            consecutive_pongs: 0,
            max_missed_streak: 0,
        }
    }

    pub fn record_rtt(&mut self, rtt: Duration) {
        self.rtt_samples.push_back(rtt);
        if self.rtt_samples.len() > 10 {
            self.rtt_samples.pop_front();
        }

        // Calculate rolling average
        let sum: Duration = self.rtt_samples.iter().sum();
        self.avg_rtt = Some(sum / self.rtt_samples.len() as u32);
    }

    pub fn quality_score(&self) -> f64 {
        if self.pings_sent == 0 {
            return 1.0;
        }

        let response_rate = self.pongs_received as f64 / self.pings_sent as f64;
        let streak_penalty = (self.max_missed_streak as f64 * 0.1).min(0.5);
        let uptime_bonus = (self.connection_start.elapsed().as_secs() as f64 / 3600.0).min(0.2);

        (response_rate - streak_penalty + uptime_bonus).clamp(0.0, 1.0)
    }

    pub fn uptime(&self) -> Duration {
        self.connection_start.elapsed()
    }

    pub fn success_rate(&self) -> f64 {
        if self.pings_sent == 0 {
            1.0
        } else {
            self.pongs_received as f64 / self.pings_sent as f64
        }
    }
}
