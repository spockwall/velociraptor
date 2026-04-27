use super::{exit_if_empty, load_section_or_exit};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pss::SigningKey;
use rsa::rand_core::OsRng;
use rsa::sha2::Sha256;
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::RsaPrivateKey;
use serde::Deserialize;
use std::path::Path;

/// Kalshi API credentials + RSA-PSS request signer.
///
/// Header layout (sent on every authenticated REST/WS request):
///
/// | Header                   | Value                                                    |
/// |--------------------------|----------------------------------------------------------|
/// | `KALSHI-ACCESS-KEY`      | API key UUID                                             |
/// | `KALSHI-ACCESS-TIMESTAMP`| Current Unix time in milliseconds (string)               |
/// | `KALSHI-ACCESS-SIGNATURE`| base64(RSA-PSS-SHA256(`timestamp + METHOD + path`))      |
///
/// `secret` must be PKCS#8 (`-----BEGIN PRIVATE KEY-----`). PKCS#1 input is
/// also accepted for legacy keys. To convert PKCS#1:
/// `openssl pkcs8 -topk8 -nocrypt -in key.pem -out key.pkcs8.pem`.
///
/// `env` selects between `"prod"` (api.elections.kalshi.com) and `"demo"`
/// (demo-api.kalshi.co). Keys issued on one env do not work on the other.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KalshiCredentials {
    /// API key ID (UUID).
    #[serde(default)]
    pub api_key: String,
    /// RSA private key in PEM format (PKCS#8 preferred, PKCS#1 accepted).
    #[serde(default)]
    pub secret: String,
}

impl KalshiCredentials {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let creds: Self = load_section_or_exit(path, "kalshi");
        exit_if_empty("api_key", "kalshi", &creds.api_key);
        exit_if_empty("secret", "kalshi", &creds.secret);
        creds
    }

    /// Parse the PEM secret into an RSA-PSS signing key. Call once at startup
    /// and reuse — parsing is the expensive part, signing is cheap.
    pub fn signing_key(&self) -> Result<SigningKey<Sha256>> {
        let rsa_key = RsaPrivateKey::from_pkcs8_pem(&self.secret)
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(&self.secret))
            .map_err(|e| anyhow!("Kalshi: failed to parse RSA private key: {e}"))?;
        Ok(SigningKey::<Sha256>::new(rsa_key))
    }

    /// Build the three `KALSHI-ACCESS-*` header tuples for one request.
    /// `method` is the upper-case HTTP verb; `path` is the URL path including
    /// any leading slash (e.g. `"/trade-api/v2/portfolio/orders"`).
    /// Call fresh per request — the timestamp and signature are time-bounded.
    pub fn build_headers(
        &self,
        signing_key: &SigningKey<Sha256>,
        method: &str,
        path: &str,
    ) -> Vec<(String, String)> {
        let ts_ms = Utc::now().timestamp_millis().to_string();
        let msg = format!("{ts_ms}{method}{path}");
        let sig = signing_key.sign_with_rng(&mut OsRng, msg.as_bytes());
        vec![
            ("KALSHI-ACCESS-KEY".into(), self.api_key.clone()),
            ("KALSHI-ACCESS-TIMESTAMP".into(), ts_ms),
            (
                "KALSHI-ACCESS-SIGNATURE".into(),
                BASE64.encode(sig.to_bytes()),
            ),
        ]
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
        assert!(!creds.api_key.is_empty());
        assert!(!creds.secret.is_empty());
    }
}
