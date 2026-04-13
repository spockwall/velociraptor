use super::{PAUSE_DELAY, SystemControl};
use crate::connection::{BasicConnectionMsgTrait, ConnectionConfig, MessageParserTrait};
use crate::heartbeat::{HealthStatus, HearthbeatConfig, HearthbeatManager, HearthbeatProtocol};
use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, interval, sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;

pub struct ConnectionBase<P: MessageParserTrait<M>, M: BasicConnectionMsgTrait> {
    config: ConnectionConfig,
    message_tx: UnboundedSender<M>,
    message_parser: P,
    reconnect_attempts: u32,
    hearthbeat_manager: HearthbeatManager<M>,
    system_control: SystemControl,
    exchange_name: ExchangeName,
    pre_subscription_messages: Vec<String>,
    post_subscription_messages: Vec<String>,
}

impl<P: MessageParserTrait<M>, M: BasicConnectionMsgTrait> ConnectionBase<P, M> {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<M>,
        system_control: SystemControl,
        message_parser: P,
        exchange_name: ExchangeName,
        pre_subscription_messages: Vec<String>,
        post_subscription_messages: Vec<String>,
    ) -> Self {
        let hearthbeat_config = HearthbeatConfig {
            ping_interval: Duration::from_secs(config.ping_interval),
            pong_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(10),
            max_missed_pongs: 2,
            ..Default::default() // Use simplified defaults
        };

        let hearthbeat_manager = HearthbeatManager::new(
            hearthbeat_config,
            message_tx.clone(),
            config.exchange.to_string(),
        );

        Self {
            config,
            message_tx,
            message_parser,
            reconnect_attempts: 0,
            hearthbeat_manager,
            system_control,
            exchange_name,
            pre_subscription_messages,
            post_subscription_messages,
        }
    }

    pub fn get_exchange_config(&self) -> &ConnectionConfig {
        &self.config
    }

    async fn handle_message(&self, msg: Message) -> Result<()> {
        match msg {
            Message::Text(text) => match self.message_parser.parse_message(&text) {
                Ok(messages) => {
                    for message in messages {
                        if self.message_tx.send(message).is_err() {
                            error!("Failed to send message to channel: receiver dropped");
                        }
                    }
                }
                Err(e) => error!("Parse error: {}", e),
            },
            Message::Close(_) => return Err(anyhow!("Close frame received")),
            _ => debug!("Unhandled message type"),
        }
        Ok(())
    }

    pub fn get_health_status(&mut self) -> Result<&HealthStatus> {
        Ok(self.hearthbeat_manager.get_health_status())
    }

    async fn connect_and_maintain(&mut self) -> Result<()> {
        let url = Url::parse(&self.config.ws_url)?;
        info!("Connecting to {}: {url}", self.exchange_name);

        let (ws_stream, _) =
            tokio::time::timeout(Duration::from_secs(10), connect_async(url.as_str())).await??;
        let (mut ws_sink, mut ws_stream) = ws_stream.split();
        let protocol = HearthbeatProtocol::new(&self.message_parser, &self.exchange_name);

        info!("Connected to {}", self.exchange_name);
        self.reconnect_attempts = 0;
        self.hearthbeat_manager.record_connection();

        // Send pre-subscription messages
        if !self.pre_subscription_messages.is_empty() {
            for msg in &self.pre_subscription_messages {
                ws_sink.send(Message::Text(msg.clone().into())).await?;
                debug!(
                    "Sent {} pre-subscription message: {msg}",
                    self.exchange_name
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Subscribe to topics
        if !self.config.subscription_message.is_empty() {
            ws_sink
                .send(Message::Text(
                    self.config.subscription_message.clone().into(),
                ))
                .await?;
            debug!(
                "Sent {} subscription for {:?}",
                self.exchange_name, self.config.subscription_message
            );
        }

        // Send post-subscription messages
        for msg in &self.post_subscription_messages {
            ws_sink.send(Message::Text(msg.clone().into())).await?;
            debug!(
                "Sent {} post-subscription message: {msg}",
                self.exchange_name
            );
        }

        let mut ping_interval = interval(self.hearthbeat_manager.get_ping_interval());
        let mut health_check_interval =
            interval(self.hearthbeat_manager.get_health_check_interval());

        loop {
            // Check shutdown first
            if self.system_control.is_shutdown() {
                info!("{} connection shutdown requested", self.exchange_name);
                return Ok(());
            }

            if self.system_control.is_paused() {
                info!("{} connection paused", self.exchange_name);
                sleep(Duration::from_millis(PAUSE_DELAY)).await;
                continue;
            }

            tokio::select! {
                // Ping timer
                _ = ping_interval.tick() => {
                    let ping_msg = protocol.build_ping();
                    if let Err(e) = ws_sink.send(ping_msg).await {
                        error!("Failed to send ping: {e}");
                    } else {
                        self.hearthbeat_manager.record_ping_sent();
                    }
                }

                                // Health check
                _ = health_check_interval.tick() => {
                    match self.hearthbeat_manager.check_health() {
                        Ok(health) => {
                            // Log connection stats periodically for stable connections
                            if self.hearthbeat_manager.is_stable_connection() {
                                let stats = self.hearthbeat_manager.get_stats();
                                if stats.total_pings % 20 == 0 { // Every 20 pings for stable connections
                                    info!("{} connection stats: {}", self.exchange_name, stats.format_summary());
                                }
                            }

                            // Simple health-based reconnection logic
                            if let crate::heartbeat::state::HealthStatus::Critical { missed_count, .. } = health {
                                if missed_count >= 3 { // Simple threshold
                                    return Err(anyhow!("Heartbeat failure - connection unhealthy"));
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }

                // WebSocket messages
                ws_msg = ws_stream.next() => {
                    match ws_msg {
                        Some(Ok(msg)) => {
                            // Check ping/pong first
                            if protocol.is_pong(&msg) {
                                self.hearthbeat_manager.record_pong();
                                continue;
                            }

                            if protocol.is_ping(&msg) {
                                debug!("Received ping, auto-pong sent by tungstenite");
                                continue;
                            }

                            // Handle other messages
                            if let Err(e) = self.handle_message(msg).await {
                                error!("Error handling {} message: {}", self.exchange_name, e);
                            }
                        }
                        Some(Err(e)) => return Err(anyhow!("WebSocket error: {}", e)),
                        None => return Err(anyhow!("WebSocket closed")),
                    }
                }
            }
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Starting {} connection", self.exchange_name);

        loop {
            // Check shutdown before attempting connection
            if self.system_control.is_shutdown() {
                info!(
                    "{} connection shutdown requested before connection",
                    self.exchange_name
                );
                break;
            }

            match self.connect_and_maintain().await {
                Ok(_) => {
                    info!("{} connection ended gracefully", self.exchange_name);
                    break;
                }
                Err(e) => {
                    error!("{} connection error: {}", self.exchange_name, e);
                    // set pause to true
                    self.system_control.pause();

                    if self.reconnect_attempts >= self.config.max_reconnect_attempts {
                        error!("{} max reconnection attempts reached", self.exchange_name);
                        return Err(anyhow!("Max reconnection attempts exceeded"));
                    }

                    self.reconnect_attempts += 1;
                    warn!(
                        "{} reconnecting in {} seconds (attempt {}/{})",
                        self.exchange_name,
                        self.config.reconnect_delay,
                        self.reconnect_attempts,
                        self.config.max_reconnect_attempts
                    );

                    let _ = self.message_tx.send(M::disconnected());
                    sleep(Duration::from_secs(self.config.random_reconnect_delay())).await;
                    self.system_control.resume();
                }
            }
        }

        Ok(())
    }
}
