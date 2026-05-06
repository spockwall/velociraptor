use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

/// Polymarket CLOB v2 credentials.
///
/// `api_key` / `secret` / `passphrase` — derived L2 API trio used to sign every
/// REST request via HMAC-SHA256 (`secret` is base64-encoded).
///
/// `address` — the Polygon address that owns the API key (sent as `POLY_ADDRESS`).
///
/// `eth_priv_key` — hex-encoded secp256k1 private key for the maker. Used to
/// EIP-712-sign the `Order` struct on every Place. Optional only because some
/// read-only setups don't need it; trading flows require it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolymarketCredentials {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Maker EOA address (0x-prefixed, 20 bytes). Empty for read-only setups.
    #[serde(default)]
    pub address: String,
    /// Maker secp256k1 private key, hex-encoded. Optional for read-only setups.
    #[serde(default)]
    pub eth_priv_key: Option<String>,
}

impl PolymarketCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "polymarket");
        exit_if_empty("api_key", "polymarket", &creds.api_key);
        exit_if_empty("secret", "polymarket", &creds.secret);
        creds
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
        let creds: PolymarketCredentials =
            serde_yaml::from_value(root.remove("polymarket").unwrap()).unwrap();
        assert!(!creds.api_key.is_empty());
        assert!(!creds.secret.is_empty());
    }
}
