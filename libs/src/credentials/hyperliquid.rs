use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HyperliquidCredentials {
    /// Hyperliquid uses wallet-based auth, but we still surface an api_key
    /// field so the same "unset" check works for every exchange.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
}

impl HyperliquidCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "hyperliquid");
        exit_if_empty("api_key", "hyperliquid", &creds.api_key);
        exit_if_empty("secret", "hyperliquid", &creds.secret);
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
    fn section_absent_defaults_to_empty() {
        // example.toml has no [hyperliquid] section.
        let raw = std::fs::read_to_string(example_toml()).unwrap();
        let mut root: toml::Table = toml::from_str(&raw).unwrap();
        let creds: HyperliquidCredentials = root
            .remove("hyperliquid")
            .map(|v| v.try_into().unwrap())
            .unwrap_or_default();
        assert!(creds.api_key.is_empty());
    }
}
