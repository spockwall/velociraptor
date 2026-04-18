use super::{PAUSE_DELAY, SystemControl};
use crate::connection::{AuthHeader, BasicClientMsgTrait, ClientConfig, MsgParserTrait};
use crate::heartbeat::{HealthStatus, HearthbeatConfig, HearthbeatManager, HearthbeatProtocol};
use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use libs::protocol::ExchangeName;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, interval, sleep};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;

pub struct ClientBase<P: MsgParserTrait<M>, M: BasicClientMsgTrait> {
    config: ClientConfig,
    message_tx: UnboundedSender<M>,
    message_parser: P,
    reconnect_attempts: u32,
    hearthbeat_manager: HearthbeatManager<M>,
    system_control: SystemControl,
    exchange_name: ExchangeName,
    /// Optional HTTP header injector for exchanges with signed upgrade requests.
    /// `None` → plain `connect_async`; `Some(b)` → b.build_headers() is called
    /// on the tungstenite `Request` before every connect attempt.
    auth_header: Option<AuthHeader>,
}

impl<P: MsgParserTrait<M>, M: BasicClientMsgTrait> ClientBase<P, M> {
    /// Create a `ConnectionBase` without custom HTTP headers (most exchanges).
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<M>,
        system_control: SystemControl,
        message_parser: P,
        exchange_name: ExchangeName,
        auth_header: Option<AuthHeader>,
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
            auth_header,
        }
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

        // The WS upgrade is an HTTP GET with Upgrade/Connection headers; any
        // exchange-specific auth headers (e.g. Kalshi's signed triple) ride on
        // that same request.
        let mut request = url.as_str().into_client_request()?;
        if let Some(ref headers) = self.auth_header {
            let h = request.headers_mut();
            for (name, value) in headers {
                let v = HeaderValue::from_str(value)
                    .map_err(|e| anyhow!("invalid header value for {name}: {e}"))?;
                h.insert(
                    tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(
                        name.as_bytes(),
                    )
                    .map_err(|e| anyhow!("invalid header name {name}: {e}"))?,
                    v,
                );
            }
            debug!("{} auth headers injected", self.exchange_name);
        }

        let (ws_stream, _) =
            tokio::time::timeout(Duration::from_secs(10), connect_async(request)).await??;
        let (mut ws_sink, mut ws_stream) = ws_stream.split();
        let protocol = HearthbeatProtocol::new(&self.message_parser, &self.exchange_name);

        info!("Connected to {}", self.exchange_name);
        self.reconnect_attempts = 0;
        self.hearthbeat_manager.record_connection();

        // Subscribe to topics — one frame per entry.
        for msg in &self.config.subscription_messages {
            if !msg.is_empty() {
                ws_sink.send(Message::Text(msg.clone().into())).await?;
                debug!("Sent {} subscription: {msg}", self.exchange_name);
            }
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
                                // Every 20 pings for stable connections
                                if stats.total_pings % 20 == 0 {
                                    info!("{} connection stats: {}", self.exchange_name, stats.format_summary());
                                }
                            }

                            // Simple health-based reconnection logic
                            if let crate::heartbeat::state::HealthStatus::Critical { missed_count, .. } = health {
                                // Simple threshold
                                if missed_count >= 3 {
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
