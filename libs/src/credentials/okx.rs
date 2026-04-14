use super::{exit_if_empty, load_section_or_exit};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OkxCredentials {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub secret: String,
    /// OKX requires a passphrase for private endpoints.
    #[serde(default)]
    pub passphrase: Option<String>,
}

impl OkxCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "okx");
        exit_if_empty("api_key", "okx", &creds.api_key);
        exit_if_empty("secret", "okx", &creds.secret);
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
        // example.toml has no [okx] section — deserialization falls back to Default.
        let raw = std::fs::read_to_string(example_toml()).unwrap();
        let mut root: toml::Table = toml::from_str(&raw).unwrap();
        let creds: OkxCredentials = root
            .remove("okx")
            .map(|v| v.try_into().unwrap())
            .unwrap_or_default();
        assert!(creds.api_key.is_empty());
    }
}
