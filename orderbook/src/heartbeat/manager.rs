use crate::connection::BasicConnectionMsgTrait;
use crate::heartbeat::HearthbeatConfig;
use crate::heartbeat::metrics::ConnectionMetrics;
use crate::heartbeat::state::{ConnectionStats, HealthStatus, HearthbeatState};
use anyhow::Result;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, error, info, warn};

/// Enhanced manager with metrics
pub struct HearthbeatManager<M: BasicConnectionMsgTrait> {
    state: HearthbeatState,
    config: HearthbeatConfig,
    message_tx: UnboundedSender<M>,
    exchange_name: String,
    pub health_status: HealthStatus,
}

impl<M: BasicConnectionMsgTrait> HearthbeatManager<M> {
    pub fn new(
        config: HearthbeatConfig,
        message_tx: UnboundedSender<M>,
        exchange_name: String,
    ) -> Self {
        Self {
            state: HearthbeatState::new(config.max_missed_pongs),
            health_status: HealthStatus::Healthy {
                rtt: None,
                quality_score: 1.0,
            },
            config,
            message_tx,
            exchange_name,
        }
    }

    pub fn record_connection(&mut self) {
        info!("{} connection established", self.exchange_name);
        self.state.reset_connection();
        let _ = self.message_tx.send(M::connected());
    }

    pub fn record_pong(&mut self) {
        debug!(
            "{} pong received (RTT: {:?})",
            self.exchange_name,
            self.state.last_ping_sent.map(|t| t.elapsed())
        );
        self.state.record_pong();
        let _ = self.message_tx.send(M::pong());
    }

    pub fn record_ping_sent(&mut self) {
        debug!("{} ping sent", self.exchange_name);
        self.state.record_ping_sent();
        let _ = self.message_tx.send(M::ping());
    }

    /// Enhanced health check with adaptive timeout
    pub fn check_health(&mut self) -> Result<HealthStatus> {
        let health = self
            .state
            .check_health(self.config.pong_timeout, &self.config);

        match &health {
            HealthStatus::Healthy { rtt, quality_score } => {
                debug!(
                    "{} healthy - RTT: {:?}, Quality: {:.2}",
                    self.exchange_name, rtt, quality_score
                );
                self.health_status = HealthStatus::Healthy {
                    rtt: *rtt,
                    quality_score: *quality_score,
                };
            }
            HealthStatus::Warning {
                elapsed,
                missed_count,
                quality_score,
            } => {
                warn!(
                    "{} warning - Last pong: {:?} ago, Missed: {}, Quality: {:.2}",
                    self.exchange_name, elapsed, missed_count, quality_score
                );
                self.health_status = HealthStatus::Warning {
                    elapsed: *elapsed,
                    missed_count: *missed_count,
                    quality_score: *quality_score,
                };
            }
            HealthStatus::Critical {
                elapsed,
                missed_count,
            } => {
                error!(
                    "{} critical - Last pong: {:?} ago, Missed: {}",
                    self.exchange_name, elapsed, missed_count
                );
                self.health_status = HealthStatus::Critical {
                    elapsed: *elapsed,
                    missed_count: *missed_count,
                };
            }
        }
        Ok(self.health_status.clone())
    }

    /// Get current connection metrics
    pub fn get_metrics(&self) -> &ConnectionMetrics {
        self.state.get_metrics()
    }

    /// Get detailed connection statistics
    pub fn get_stats(&self) -> ConnectionStats {
        self.state.get_stats(self.config.pong_timeout, &self.config)
    }

    /// Check if connection should be considered stable
    pub fn is_stable_connection(&self) -> bool {
        self.state.is_stable_connection()
    }

    /// Get ping interval (fixed, no adaptive frequency)
    pub fn get_ping_interval(&self) -> Duration {
        self.config.ping_interval
    }

    /// Get pong timeout (potentially adaptive based on RTT)
    pub fn get_pong_timeout(&self) -> Duration {
        if self.config.adaptive_timeout {
            if let Some(avg_rtt) = self.state.metrics.avg_rtt {
                let adaptive = Duration::from_millis(
                    (avg_rtt.as_millis() as f64 * self.config.adaptive_multiplier) as u64,
                );
                adaptive.clamp(
                    self.config.min_adaptive_timeout,
                    self.config.max_adaptive_timeout,
                )
            } else {
                self.config.pong_timeout
            }
        } else {
            self.config.pong_timeout
        }
    }

    pub fn get_health_check_interval(&self) -> Duration {
        self.config.health_check_interval
    }

    pub fn get_health_status(&self) -> &HealthStatus {
        &self.health_status
    }
}
