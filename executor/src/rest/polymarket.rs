//! Polymarket CLOB v2 REST order client — backed by the official
//! `polymarket_client_sdk_v2` crate.
//!
//! All wire-level concerns (L2 HMAC auth, EIP-712 v2 order signing, JSON
//! envelope shape, error code mapping) are delegated to the upstream SDK.
//! This module is a thin adapter that:
//!
//!   1. Builds an authenticated SDK `Client<Authenticated<Normal>>` from
//!      `libs::credentials::PolymarketCredentials` (only `eth_priv_key` is
//!      consumed; the SDK derives the L2 trio internally on `authenticate()`).
//!   2. Implements `RestOrderClient` by translating our generic `PlaceOne`
//!      / `OrderAck` types into the SDK's order builder + response types.
//!
//! The hand-rolled HMAC + EIP-712 signing path is gone (was `eip712.rs` +
//! 524-LOC `polymarket.rs`). The SDK owns wire correctness now.

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use libs::credentials::polymarket::PolymarketCredentials;
use libs::protocol::orders::{
    HeartbeatAck, OrderAck, OrderError, OrderKind, OrderStatus, PlaceOne, Side, Tif,
};
use polymarket_client_sdk_v2::auth::state::Authenticated;
use polymarket_client_sdk_v2::auth::{LocalSigner, Normal, Signer};
// `LocalSigner` is generic; `from_str` produces this concrete shape (k256
// ECDSA key) — same as `alloy_signer_local::PrivateKeySigner`.
type PrivKeySigner = LocalSigner<k256::ecdsa::SigningKey>;
use polymarket_client_sdk_v2::clob::types::request::{CancelMarketOrderRequest, OrdersRequest};
use polymarket_client_sdk_v2::clob::types::{
    Amount, OrderStatusType, OrderType, Side as SdkSide, SignatureType,
};
use polymarket_client_sdk_v2::clob::{Client, Config};
use polymarket_client_sdk_v2::types::{Address, Decimal, U256};
use polymarket_client_sdk_v2::{derive_proxy_wallet, derive_safe_wallet};

use super::{RestOrderClient, TargetPrice};
use crate::error::map_internal;

/// Polygon mainnet chain id — required by `LocalSigner` for the v2 EIP-712
/// domain.
const POLYGON: u64 = 137;

pub struct PolymarketRestClient {
    /// Authenticated SDK client. `Arc` so per-request futures can hold a
    /// cheap clone (the trait takes `&self`, methods are async).
    client: Arc<Client<Authenticated<Normal>>>,
    /// EOA signer kept on the side because `OrderBuilder::build_sign_and_post`
    /// needs `&S: Signer` per call. The SDK does NOT retain the signer on
    /// `Client` after `authenticate()` — it only keeps the derived L2
    /// `Credentials`. Storing here means signing on the hot path doesn't
    /// re-read the yaml.
    signer: Arc<PrivKeySigner>,
    /// Public Gamma host stays here for `target_price()` — the SDK's
    /// market-discovery surface is shaped differently and `RestOrderClient`
    /// callers rely on this exact accessor.
    http: reqwest::Client,
}

impl PolymarketRestClient {
    /// Construct an authenticated client. Async because the SDK's
    /// `authenticate()` performs an HTTP round-trip to derive (or fetch) the
    /// L2 api key/secret/passphrase from the EOA signer.
    pub async fn new(
        creds: PolymarketCredentials,
        base_url: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let priv_key = creds
            .eth_priv_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("polymarket: eth_priv_key required"))?;
        let signer: PrivKeySigner = LocalSigner::from_str(priv_key)
            .map_err(|e| anyhow::anyhow!("polymarket signer parse: {e}"))?;
        let signer = signer.with_chain_id(Some(POLYGON));

        // Resolve signature flavour. Polymarket retail accounts created via
        // the web UI hold funds in a Gnosis Safe / proxy contract — the EOA
        // can only sign, never be the order `maker`. Submitting a raw-EOA
        // order against such an account 400s with "maker address not allowed,
        // please use the deposit wallet flow". Setting `signature_type` +
        // `funder` on the auth builder switches the SDK into proxy mode.
        let sig_type = parse_signature_type(creds.signature_type.as_deref())?;
        let funder = resolve_funder(&creds, &signer, sig_type)?;

        let unauth = Client::new(&base_url.into(), Config::default())
            .map_err(|e| anyhow::anyhow!("polymarket sdk Client::new: {e}"))?;
        let mut auth = unauth.authentication_builder(&signer);
        if let Some(t) = sig_type {
            auth = auth.signature_type(t);
        }
        if let Some(f) = funder {
            auth = auth.funder(f);
        }
        let client = auth
            .authenticate()
            .await
            .map_err(|e| anyhow::anyhow!("polymarket sdk authenticate: {e}"))?;

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client: Arc::new(client),
            signer: Arc::new(signer),
            http,
        })
    }

    /// Fetch the strike `line` (and optional bounds) for a market by slug
    /// from Polymarket's public Gamma API. Unauthenticated. Kept here
    /// because the SDK's market-discovery surface is shaped differently and
    /// `RestOrderClient` callers rely on this exact accessor.
    pub async fn target_price(&self, slug: &str) -> Result<TargetPrice, OrderError> {
        use libs::endpoints::polymarket::polymarket::gamma;
        let url = format!("{}{}{}", gamma::BASE_URL, gamma::MARKET_BY_SLUG, slug);
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| OrderError::Network {
                message: e.to_string(),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| OrderError::Network {
            message: e.to_string(),
        })?;
        if !status.is_success() {
            return Err(OrderError::ExchangeRejected {
                code: Some(status.as_u16().to_string()),
                message: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(map_internal)?;
        let f = |k: &str| {
            body.get(k).and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_f64(),
                serde_json::Value::String(s) => s.parse::<f64>().ok(),
                _ => None,
            })
        };
        Ok(TargetPrice {
            line: f("line"),
            lower: f("lowerBound"),
            upper: f("upperBound"),
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse `signature_type` from credentials. Accepts `eoa`, `proxy`,
/// `gnosis_safe` (case-insensitive). Returns `Ok(None)` when unset to mean
/// "let the SDK pick its default (Eoa)".
fn parse_signature_type(s: Option<&str>) -> anyhow::Result<Option<SignatureType>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "eoa" => Ok(Some(SignatureType::Eoa)),
            "proxy" | "poly_proxy" | "polyproxy" => Ok(Some(SignatureType::Proxy)),
            "gnosis_safe" | "gnosissafe" | "safe" => Ok(Some(SignatureType::GnosisSafe)),
            other => Err(anyhow::anyhow!(
                "polymarket: unknown signature_type {other:?} (expected eoa|proxy|gnosis_safe)"
            )),
        },
    }
}

/// Pick the funder address. Priority:
///   1. `creds.funder` if set (parsed as 0x-hex).
///   2. For Proxy/GnosisSafe sig types, derive deterministically from the
///      signer's EOA via the SDK helpers.
///   3. Otherwise `None` (raw EOA flow — SDK uses signer.address() as maker).
fn resolve_funder(
    creds: &PolymarketCredentials,
    signer: &PrivKeySigner,
    sig_type: Option<SignatureType>,
) -> anyhow::Result<Option<Address>> {
    if let Some(f) = creds.funder.as_deref().filter(|s| !s.is_empty()) {
        let addr = Address::from_str(f)
            .map_err(|e| anyhow::anyhow!("polymarket: funder parse {f:?}: {e}"))?;
        return Ok(Some(addr));
    }
    let eoa = signer.address();
    match sig_type {
        Some(SignatureType::Proxy) => Ok(derive_proxy_wallet(eoa, POLYGON)),
        Some(SignatureType::GnosisSafe) => Ok(derive_safe_wallet(eoa, POLYGON)),
        _ => Ok(None),
    }
}

fn map_sdk_err(e: impl std::fmt::Display) -> OrderError {
    // The SDK's error type is opaque enough that "ExchangeRejected" is the
    // most truthful default. If we need finer granularity later we can match
    // on `polymarket_client_sdk_v2::error::Error` variants explicitly.
    OrderError::ExchangeRejected {
        code: None,
        message: e.to_string(),
    }
}

fn dec(v: f64) -> Result<Decimal, OrderError> {
    Decimal::try_from(v).map_err(|e| OrderError::Internal {
        message: format!("polymarket: bad decimal {v}: {e}"),
    })
}

fn parse_token_id(s: &str) -> Result<U256, OrderError> {
    U256::from_str(s).map_err(|e| OrderError::Internal {
        message: format!("polymarket: token_id parse failed: {e}"),
    })
}

fn to_sdk_side(s: Side) -> SdkSide {
    match s {
        Side::Buy => SdkSide::Buy,
        Side::Sell => SdkSide::Sell,
    }
}

fn to_sdk_order_type(t: Tif) -> OrderType {
    match t {
        Tif::Gtc => OrderType::GTC,
        Tif::Gtd => OrderType::GTD,
        // CLOB v2 has FAK as well — keep IOC mapped to FOK for now (the
        // existing semantic in our previous impl); revisit if FAK is the
        // closer match for callers' intent.
        Tif::Ioc => OrderType::FOK,
        Tif::Fok => OrderType::FOK,
    }
}

// Market orders on Polymarket CLOB v2 only accept FAK or FOK.
fn to_sdk_market_order_type(t: Tif) -> Result<OrderType, OrderError> {
    match t {
        Tif::Fok => Ok(OrderType::FOK),
        Tif::Ioc => Ok(OrderType::FAK),
        Tif::Gtc | Tif::Gtd => Err(OrderError::Internal {
            message: format!("polymarket market order: unsupported tif {t:?} (use IOC/FOK)"),
        }),
    }
}

// `qty` is share count for both sides:
//   Buy  → walks asks until cumulative shares >= qty
//   Sell → walks bids until cumulative shares >= qty
fn to_market_amount(qty: f64) -> Result<Amount, OrderError> {
    Amount::shares(dec(qty)?).map_err(|e| OrderError::Internal {
        message: format!("polymarket market order: amount build failed: {e}"),
    })
}

fn map_status(s: &OrderStatusType) -> OrderStatus {
    match s {
        OrderStatusType::Live => OrderStatus::New,
        OrderStatusType::Matched => OrderStatus::PartiallyFilled,
        OrderStatusType::Canceled => OrderStatus::Canceled,
        OrderStatusType::Delayed => OrderStatus::New,
        OrderStatusType::Unmatched => OrderStatus::New,
        OrderStatusType::Unknown(s) => match s.to_uppercase().as_str() {
            "FILLED" | "EXECUTED" => OrderStatus::Filled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" => OrderStatus::Expired,
            _ => OrderStatus::New,
        },
        // `OrderStatusType` is `#[non_exhaustive]` — future SDK variants land
        // here. Default to `New` rather than panicking; the gateway logs it.
        _ => OrderStatus::New,
    }
}

fn now_ns() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

// ── Trait impl ──────────────────────────────────────────────────────────────

#[async_trait]
impl RestOrderClient for PolymarketRestClient {
    async fn place(&self, p: &PlaceOne) -> Result<OrderAck, OrderError> {
        // Canonical SDK flow: `limit_order()` / `market_order()` -> typestate
        // builder -> `build_sign_and_post(&signer)`. The SDK handles the
        // EIP-712 v2 domain, struct hash, secp256k1 signing, and JSON envelope.
        let resp = match p.kind {
            OrderKind::Limit => self
                .client
                .limit_order()
                .token_id(parse_token_id(&p.symbol)?)
                .side(to_sdk_side(p.side))
                .price(dec(p.px)?)
                .size(dec(p.qty)?)
                .order_type(to_sdk_order_type(p.tif))
                .build_sign_and_post(self.signer.as_ref())
                .await
                .map_err(map_sdk_err)?,
            OrderKind::Market => self
                .client
                .market_order()
                .token_id(parse_token_id(&p.symbol)?)
                .side(to_sdk_side(p.side))
                .amount(to_market_amount(p.qty)?)
                .order_type(to_sdk_market_order_type(p.tif)?)
                .build_sign_and_post(self.signer.as_ref())
                .await
                .map_err(map_sdk_err)?,
        };

        Ok(OrderAck {
            client_oid: p.client_oid.clone(),
            exchange_oid: resp.order_id,
            status: map_status(&resp.status),
            ts_ns: now_ns(),
        })
    }

    async fn place_batch(
        &self,
        os: &[PlaceOne],
    ) -> Result<Vec<Result<OrderAck, OrderError>>, OrderError> {
        // The SDK's `post_orders(Vec<SignedOrder>)` requires us to construct
        // each `SignedOrder` ourselves first; per-leg failure isolation is
        // simpler if we just iterate `place()`. Network cost is the same
        // (one HTTP per order under either path).
        let mut out = Vec::with_capacity(os.len());
        for p in os {
            out.push(self.place(p).await);
        }
        Ok(out)
    }

    async fn update(
        &self,
        _client_oid: &str,
        _exchange_oid: &str,
        _new_px: Option<f64>,
        _new_qty: Option<f64>,
    ) -> Result<OrderAck, OrderError> {
        Err(OrderError::Internal {
            message: "polymarket.update: no native amend on CLOB v2; emulate via cancel+place"
                .into(),
        })
    }

    async fn cancel(&self, exchange_oid: &str) -> Result<OrderAck, OrderError> {
        let _ = self
            .client
            .cancel_order(exchange_oid)
            .await
            .map_err(map_sdk_err)?;
        Ok(OrderAck {
            client_oid: String::new(),
            exchange_oid: exchange_oid.to_string(),
            status: OrderStatus::Canceled,
            ts_ns: now_ns(),
        })
    }

    async fn cancel_all(&self) -> Result<u32, OrderError> {
        let resp = self.client.cancel_all_orders().await.map_err(map_sdk_err)?;
        Ok(resp.canceled.len() as u32)
    }

    async fn cancel_market(&self, symbol: &str) -> Result<u32, OrderError> {
        // CLOB v2's `/cancel-market-orders` accepts either a market condition
        // id (B256) OR an asset_id (U256). `RestOrderClient::cancel_market`
        // takes a generic `symbol` string — try as token id (decimal U256)
        // since that's what `PlaceOne.symbol` carries.
        let asset_id = parse_token_id(symbol)?;
        // `CancelMarketOrderRequest` is `#[non_exhaustive]`; use its derived
        // builder instead of struct-literal syntax.
        let req = CancelMarketOrderRequest::builder()
            .asset_id(asset_id)
            .build();
        let resp = self
            .client
            .cancel_market_orders(&req)
            .await
            .map_err(map_sdk_err)?;
        Ok(resp.canceled.len() as u32)
    }

    async fn get_order(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError> {
        self.order_status(exchange_oid).await
    }

    async fn get_orders(&self) -> Result<Vec<OrderAck>, OrderError> {
        let req = OrdersRequest::default();
        let page = self.client.orders(&req, None).await.map_err(map_sdk_err)?;
        Ok(page
            .data
            .into_iter()
            .map(|o| OrderAck {
                client_oid: String::new(),
                exchange_oid: o.id,
                status: map_status(&o.status),
                ts_ns: now_ns(),
            })
            .collect())
    }

    async fn order_status(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError> {
        let o = self.client.order(exchange_oid).await.map_err(map_sdk_err)?;
        Ok(map_status(&o.status))
    }

    async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError> {
        // CLOB v2's `/v1/heartbeats` has no next-due semantics. The SDK
        // gates heartbeat support behind a feature we don't enable, so
        // return a no-op ack that the gateway treats as "always alive."
        Ok(HeartbeatAck {
            next_due_ms: u64::MAX,
        })
    }
}
