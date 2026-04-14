use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct KalshiCredentials {
    #[serde(default)]
    pub api_key: String,
}

impl KalshiCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "kalshi");
        exit_if_empty("api_key", "kalshi", &creds.api_key);
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
        let creds: KalshiCredentials = root.remove("kalshi").unwrap().try_into().unwrap();
        assert!(!creds.api_key.is_empty());
    }
}
