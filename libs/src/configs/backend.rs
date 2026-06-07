use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    pub port: u16,
    /// Root directory whose immediate subfolders the monitor reports
    /// directory (du-style) usage for. On the server this is the data volume
    /// (`/data`); on a dev box without it the monitor's `data_usage` is just
    /// empty (no error).
    pub data_dir: String,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            data_dir: "/data".to_string(),
        }
    }
}
