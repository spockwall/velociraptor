use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HyperliquidCredentials {
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
        let creds: HyperliquidCredentials =
            serde_yaml::from_value(root.remove("hyperliquid").unwrap()).unwrap();
        assert!(!creds.api_key.is_empty());
        assert!(!creds.secret.is_empty());
    }
}
