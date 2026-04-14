use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolymarketCredentials {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
    /// Polymarket CLOB signing passphrase (L2 header).
    #[serde(default)]
    pub passphrase: Option<String>,
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

    fn example_toml() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("credentials/example.toml")
    }

    #[test]
    fn deserializes_from_example() {
        let raw = std::fs::read_to_string(example_toml()).unwrap();
        let mut root: toml::Table = toml::from_str(&raw).unwrap();
        let creds: PolymarketCredentials = root.remove("polymarket").unwrap().try_into().unwrap();
        assert!(!creds.api_key.is_empty());
        assert!(!creds.secret.is_empty());
    }
}
