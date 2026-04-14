use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub pub_endpoint: String,
    pub router_endpoint: String,
    pub render_interval: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            pub_endpoint: "tcp://*:5555".to_string(),
            router_endpoint: "tcp://*:5556".to_string(),
            render_interval: 300,
        }
    }
}
