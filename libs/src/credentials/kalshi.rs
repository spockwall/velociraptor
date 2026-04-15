use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

/// Kalshi API credentials.
///
/// `api_key` — the UUID shown in the Kalshi API keys page (sent as `KALSHI-ACCESS-KEY`)
/// `secret`  — the RSA private key PEM string used to sign each request
///             (RSA-PSS with SHA-256; the public key is registered on Kalshi).
///             Must be PKCS#8 (`-----BEGIN PRIVATE KEY-----`). If Kalshi gave you
///             a PKCS#1 file (`-----BEGIN RSA PRIVATE KEY-----`), convert with:
///               `openssl pkcs8 -topk8 -nocrypt -in key.pem -out key.pkcs8.pem`
/// `env`     — `"prod"` (default, api.elections.kalshi.com) or `"demo"`
///             (demo-api.kalshi.co). Keys issued on one env do not work on the other.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KalshiCredentials {
    /// API key ID (UUID), e.g. `"a1b2c3d4-e5f6-..."`
    #[serde(default)]
    pub api_key: String,
    /// RSA private key in PEM format (PKCS#8).
    /// Used to sign WebSocket upgrade requests with RSA-PSS / SHA-256.
    #[serde(default)]
    pub secret: String,
    /// Environment: `"prod"` or `"demo"`. Defaults to `"prod"`.
    #[serde(default = "default_env")]
    pub env: String,
}

fn default_env() -> String {
    "prod".to_string()
}

impl KalshiCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "kalshi");
        exit_if_empty("api_key", "kalshi", &creds.api_key);
        exit_if_empty("secret", "kalshi", &creds.secret);
        creds
    }

    /// WebSocket endpoint for the configured environment.
    pub fn ws_url(&self) -> &'static str {
        match self.env.as_str() {
            "demo" => "wss://demo-api.kalshi.co/trade-api/ws/v2",
            _ => "wss://api.elections.kalshi.com/trade-api/ws/v2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_yaml() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("credentials/example.yaml")
    }

    #[test]
    fn deserializes_from_example() {
        let raw = std::fs::read_to_string(example_yaml()).unwrap();
        let mut root: serde_yaml::Mapping = serde_yaml::from_str(&raw).unwrap();
        let creds: KalshiCredentials =
            serde_yaml::from_value(root.remove("kalshi").unwrap()).unwrap();
        println!("{:?}", creds);
        assert!(!creds.api_key.is_empty());
        assert!(!creds.secret.is_empty());
    }
}
