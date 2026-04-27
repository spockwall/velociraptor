//! Kalshi WS upgrade header builder.
//!
//! The signing primitive lives in `libs::credentials::kalshi::KalshiCredentials`.
//! This module is a thin adapter: build a `KalshiCredentials` from the
//! `(api_key, secret_pem)` pair the WS connector already has on hand, parse the
//! signing key once, and emit the three `KALSHI-ACCESS-*` headers signed over
//! `GET /trade-api/ws/v2`.

use crate::connection::AuthHeader;
use anyhow::Result;
use libs::credentials::kalshi::KalshiCredentials;
use libs::endpoints::kalshi::kalshi::ws;

/// Build the three `KALSHI-ACCESS-*` headers for one WebSocket upgrade attempt.
pub fn ws_upgrade_headers(api_key: impl Into<String>, secret_pem: &str) -> Result<AuthHeader> {
    let creds = KalshiCredentials {
        api_key: api_key.into(),
        secret: secret_pem.to_string(),
    };
    let key = creds.signing_key()?;
    Ok(creds.build_headers(&key, "GET", ws::SIGN_PATH))
}
