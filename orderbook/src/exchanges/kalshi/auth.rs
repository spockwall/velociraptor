//! Kalshi RSA-PSS request signing.
//!
//! Builds the three HTTP headers required on every Kalshi WebSocket upgrade:
//!
//! | Header                   | Value                                                          |
//! |--------------------------|----------------------------------------------------------------|
//! | `KALSHI-ACCESS-KEY`      | API key UUID                                                   |
//! | `KALSHI-ACCESS-TIMESTAMP`| Current Unix time in milliseconds (string)                     |
//! | `KALSHI-ACCESS-SIGNATURE`| base64(RSA-PSS-SHA256(`timestamp + "GET" + "/trade-api/ws/v2"`))|

use crate::connection::AuthHeader;
use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pss::SigningKey;
use rsa::rand_core::OsRng;
use rsa::sha2::Sha256;
use rsa::signature::{RandomizedSigner, SignatureEncoding};

const SIGN_PATH: &str = "/trade-api/ws/v2";

/// Signs Kalshi WebSocket upgrade requests with RSA-PSS / SHA-256.
pub struct KalshiAuth {
    api_key: String,
    signing_key: SigningKey<Sha256>,
}

impl KalshiAuth {
    /// Parse a PKCS#8 PEM private key and pair it with the key ID UUID.
    pub fn new(api_key: impl Into<String>, secret: &str) -> Result<Self> {
        // Accept both PKCS#8 (`-----BEGIN PRIVATE KEY-----`) and the older
        // PKCS#1 (`-----BEGIN RSA PRIVATE KEY-----`) — Kalshi has handed out
        // both over the years.
        let rsa_key = RsaPrivateKey::from_pkcs8_pem(secret)
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(secret))
            .map_err(|e| anyhow!("Kalshi: failed to parse RSA private key: {e}"))?;
        Ok(Self {
            api_key: api_key.into(),
            signing_key: SigningKey::<Sha256>::new(rsa_key),
        })
    }

    /// Compute `(key_id, timestamp_ms, signature_b64)` for one upgrade attempt.
    fn sign(&self) -> (String, String, String) {
        let ts_ms = Utc::now().timestamp_millis().to_string();
        let msg = format!("{ts_ms}GET{SIGN_PATH}");
        let sig = self.signing_key.sign_with_rng(&mut OsRng, msg.as_bytes());
        (self.api_key.clone(), ts_ms, BASE64.encode(sig.to_bytes()))
    }

    /// Build the three `KALSHI-ACCESS-*` headers for one upgrade attempt.
    /// Call fresh per connect — the timestamp and signature are time-bounded.
    pub fn build_headers(&self) -> AuthHeader {
        let (key, ts, sig) = self.sign();
        vec![
            ("KALSHI-ACCESS-KEY".into(), key),
            ("KALSHI-ACCESS-TIMESTAMP".into(), ts),
            ("KALSHI-ACCESS-SIGNATURE".into(), sig),
        ]
    }
}
