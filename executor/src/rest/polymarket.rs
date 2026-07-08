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
    FillInfo, HeartbeatAck, OrderAck, OrderError, OrderKind, OrderStatus, PlaceOne, Side, Tif,
};
use polymarket_client_sdk_v2::auth::state::Authenticated;
use polymarket_client_sdk_v2::auth::{Credentials, LocalSigner, Normal, Signer, Uuid};
// `LocalSigner` is generic; `from_str` produces this concrete shape (k256
// ECDSA key) — same as `alloy_signer_local::PrivateKeySigner`.
type PrivKeySigner = LocalSigner<k256::ecdsa::SigningKey>;
use polymarket_client_sdk_v2::clob::types::request::{CancelMarketOrderRequest, OrdersRequest};
use polymarket_client_sdk_v2::clob::types::response::PostOrderResponse;
use polymarket_client_sdk_v2::clob::types::{
    Amount, OrderStatusType, OrderType, Side as SdkSide, SignatureType,
};
use polymarket_client_sdk_v2::clob::{Client, Config};
use polymarket_client_sdk_v2::types::{Address, Decimal, U256};
use polymarket_client_sdk_v2::{derive_proxy_wallet, derive_safe_wallet};

use super::{record_venue_ms, RestOrderClient, TargetPrice};
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

        // Pre-existing L2 API credentials, if the creds file carries the full
        // trio. This matters for the `poly1271` deposit-wallet flow: the CLOB
        // rejects an order whose `signer` (= the deposit `funder` under
        // poly1271) doesn't match the address the *API key* is registered to
        // ("the order signer address has to be the address of the API KEY").
        // The web-UI-issued key is registered to the deposit wallet, so we
        // must present THAT key rather than letting the SDK mint a fresh one
        // bound to the bare EOA. When the trio is absent we fall back to the
        // SDK deriving a key from the EOA (correct for raw-EOA accounts).
        let explicit_creds = build_l2_credentials(&creds)?;

        let unauth = Client::new(&base_url.into(), Config::default())
            .map_err(|e| anyhow::anyhow!("polymarket sdk Client::new: {e}"))?;
        let mut auth = unauth.authentication_builder(&signer);
        if let Some(t) = sig_type {
            auth = auth.signature_type(t);
        }
        if let Some(f) = funder {
            auth = auth.funder(f);
        }
        if let Some(c) = explicit_creds {
            auth = auth.credentials(c);
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

    /// Light CLOB health-check GET issued through the SAME SDK client (and
    /// therefore the same reqwest connection pool) the order path uses. The
    /// keep-warm task in `bin/executor.rs` calls this periodically: reqwest
    /// evicts idle pool connections after 90 s, so sparse order flow would
    /// otherwise pay a fresh TCP+TLS handshake on the first order after a
    /// quiet spell (measured 459 ms first-order vs 103 ms warm on AMS).
    pub async fn warm(&self) -> Result<(), OrderError> {
        self.client
            .ok()
            .await
            .map(|_| ())
            .map_err(|e| OrderError::Network {
                message: e.to_string(),
            })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build the SDK's L2 `Credentials` from the creds file when the full trio
/// (`api_key` + `secret` + `passphrase`) is present. Returns `Ok(None)` when
/// any is missing — the caller then lets the SDK derive a fresh key from the
/// EOA (correct for raw-EOA accounts). `api_key` must be a UUID (Polymarket's
/// L2 key format); a malformed value is a hard error rather than a silent
/// fallback, so a typo can't quietly change trading identity.
fn build_l2_credentials(
    creds: &PolymarketCredentials,
) -> anyhow::Result<Option<Credentials>> {
    let passphrase = match creds.passphrase.as_deref().filter(|s| !s.is_empty()) {
        Some(p) => p,
        None => return Ok(None),
    };
    if creds.api_key.is_empty() || creds.secret.is_empty() {
        return Ok(None);
    }
    let key = Uuid::parse_str(&creds.api_key).map_err(|e| {
        anyhow::anyhow!("polymarket: api_key is not a valid UUID L2 key: {e}")
    })?;
    Ok(Some(Credentials::new(
        key,
        creds.secret.clone(),
        passphrase.to_owned(),
    )))
}

/// Parse `signature_type` from credentials. Accepts `eoa`, `proxy`,
/// `gnosis_safe`, `poly1271` (case-insensitive). Returns `Ok(None)` when unset
/// to mean "let the SDK pick its default (Eoa)".
///
/// `poly1271` (SDK `SignatureType::Poly1271`, wire value 3) is the EIP-1271
/// smart-contract-wallet flow: the order's `maker` AND `signer` are set to the
/// deposit-wallet `funder`, the EOA key produces the signature, and the CLOB
/// verifies it via the funder contract's `isValidSignature`. This is what
/// Polymarket web-UI accounts whose deposit wallet isn't CREATE2-derivable from
/// the EOA require — without it the CLOB 400s "maker address not allowed,
/// please use the deposit wallet flow". V2 orders only (the SDK rejects it on
/// V1); Polymarket prod CLOB is V2. Requires an explicit `funder` in creds
/// (there's no derivation for it).
fn parse_signature_type(s: Option<&str>) -> anyhow::Result<Option<SignatureType>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "eoa" => Ok(Some(SignatureType::Eoa)),
            "proxy" | "poly_proxy" | "polyproxy" => Ok(Some(SignatureType::Proxy)),
            "gnosis_safe" | "gnosissafe" | "safe" => Ok(Some(SignatureType::GnosisSafe)),
            "poly1271" | "poly_1271" | "eip1271" => Ok(Some(SignatureType::Poly1271)),
            other => Err(anyhow::anyhow!(
                "polymarket: unknown signature_type {other:?} \
                 (expected eoa|proxy|gnosis_safe|poly1271)"
            )),
        },
    }
}

/// Pick the funder address. Priority:
///   1. `creds.funder` if set (parsed as 0x-hex).
///   2. For Proxy/GnosisSafe sig types, derive deterministically from the
///      signer's EOA via the SDK helpers.
///   3. Otherwise `None` (raw EOA flow — SDK uses signer.address() as maker).
///
/// `Poly1271` has no CREATE2 derivation (the deposit wallet address is not a
/// deterministic function of the EOA), so it MUST carry an explicit `funder`;
/// reaching the derive branch with `Poly1271` is a config error we surface
/// early rather than letting the SDK reject it opaquely at build time.
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
        Some(SignatureType::Poly1271) => Err(anyhow::anyhow!(
            "polymarket: signature_type poly1271 requires an explicit `funder` \
             (deposit wallet address) in credentials — it cannot be derived from the EOA"
        )),
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
        // IOC on a limit = FAK: cross for whatever is displayed at or
        // inside the limit, kill the remainder. Callers that need
        // all-or-nothing pass Tif::Fok explicitly.
        Tif::Ioc => OrderType::FAK,
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

// Side-specific market-order amount semantics on Polymarket:
//
//   Buy  → `qty` is USDC NOTIONAL. The SDK back-computes share count by
//          walking the ask book; `maker = qty`, `taker = qty / cutoff_price`.
//          We must use `Amount::usdc` because the venue requires the maker
//          amount to have ≤ 2 USDC decimals — using `Amount::shares` on a
//          buy yields a fractional USDC notional that fails the precision
//          check (`maker buy orders ... max accuracy of 2 decimals`).
//
//   Sell → `qty` is SHARE COUNT. The SDK back-computes USDC by walking
//          the bid book; `maker = qty` shares, `taker = qty * cutoff_price`
//          USDC. The SDK explicitly rejects `Amount::usdc` for sells
//          (`Sell Orders must specify their amounts in shares`), so this
//          path is non-negotiable.
//
// Net effect on the trading-engine API:
//   `place_market(side="buy",  qty=N)` → spend N USDC
//   `place_market(side="sell", qty=N)` → sell N shares
fn to_market_amount(qty: f64, side: Side) -> Result<Amount, OrderError> {
    let d = dec(qty)?;
    let res = match side {
        Side::Buy => Amount::usdc(d),
        Side::Sell => Amount::shares(d),
    };
    res.map_err(|e| OrderError::Internal {
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

/// Project the SDK's `PostOrderResponse` (returned by `build_sign_and_post`)
/// onto our protocol-level [`FillInfo`]. Decimals lower to `f64` (lossless at
/// CLOB tick/size granularity); `B256` hashes render as `0x`-prefixed hex.
/// An empty `error_msg` is normalized to `None`.
fn fill_info(resp: &PostOrderResponse) -> FillInfo {
    FillInfo {
        making_amount: dec_to_f64(resp.making_amount),
        taking_amount: dec_to_f64(resp.taking_amount),
        success: resp.success,
        error_msg: resp
            .error_msg
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned(),
        transaction_hashes: resp
            .transaction_hashes
            .iter()
            .map(|h| format!("{h:#x}"))
            .collect(),
        trade_ids: resp.trade_ids.clone(),
    }
}

fn dec_to_f64(d: Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.0)
}

// ── Trait impl ──────────────────────────────────────────────────────────────

#[async_trait]
impl RestOrderClient for PolymarketRestClient {
    async fn place(&self, p: &PlaceOne) -> Result<OrderAck, OrderError> {
        // Canonical SDK flow: `limit_order()` / `market_order()` -> typestate
        // builder -> `build_sign_and_post(&signer)`. The SDK handles the
        // EIP-712 v2 domain, struct hash, secp256k1 signing, and JSON envelope.
        //
        // Latency instrumentation: we time `build_sign_and_post` with
        // `Instant` and record the elapsed ms into the request-scoped
        // `VENUE_MS` cell (see `rest::record_venue_ms`) on BOTH the accept and
        // the reject branch — a 400 (e.g. FAK "no orders found to match")
        // still pays the venue round-trip and must be measured. This is the
        // tightest boundary around the only call that reaches the CLOB;
        // `build_sign_and_post` bundles signing + the HTTP POST in the SDK, so
        // it's the finest split available without touching signing logic. No
        // order/TIF/signature semantics change — timing + a scratch write only.
        let resp = match p.kind {
            OrderKind::Limit => {
                let builder = self
                    .client
                    .limit_order()
                    .token_id(parse_token_id(&p.symbol)?)
                    .side(to_sdk_side(p.side))
                    .price(dec(p.px)?)
                    .size(dec(p.qty)?)
                    .order_type(to_sdk_order_type(p.tif));
                let started = std::time::Instant::now();
                let out = builder.build_sign_and_post(self.signer.as_ref()).await;
                record_venue_ms(started.elapsed().as_secs_f64() * 1_000.0);
                out.map_err(map_sdk_err)?
            }
            OrderKind::Market => {
                let builder = self
                    .client
                    .market_order()
                    .token_id(parse_token_id(&p.symbol)?)
                    .side(to_sdk_side(p.side))
                    .amount(to_market_amount(p.qty, p.side)?)
                    .order_type(to_sdk_market_order_type(p.tif)?);
                let started = std::time::Instant::now();
                let out = builder.build_sign_and_post(self.signer.as_ref()).await;
                record_venue_ms(started.elapsed().as_secs_f64() * 1_000.0);
                out.map_err(map_sdk_err)?
            }
        };

        Ok(OrderAck {
            client_oid: p.client_oid.clone(),
            exchange_oid: resp.order_id.clone(),
            status: map_status(&resp.status),
            ts_ns: now_ns(),
            fill: Some(fill_info(&resp)),
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
            fill: None,
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
                fill: None,
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

#[cfg(test)]
mod sig_type_tests {
    use super::*;

    #[test]
    fn parses_all_signature_flavours() {
        assert!(matches!(parse_signature_type(None).unwrap(), None));
        assert!(matches!(
            parse_signature_type(Some("eoa")).unwrap(),
            Some(SignatureType::Eoa)
        ));
        assert!(matches!(
            parse_signature_type(Some("proxy")).unwrap(),
            Some(SignatureType::Proxy)
        ));
        assert!(matches!(
            parse_signature_type(Some("gnosis_safe")).unwrap(),
            Some(SignatureType::GnosisSafe)
        ));
        // The regression this test guards: poly1271 (EIP-1271 deposit-wallet
        // flow) must parse. Its absence 400s live orders with "maker address
        // not allowed, please use the deposit wallet flow".
        for s in ["poly1271", "POLY1271", "poly_1271", "eip1271"] {
            assert!(
                matches!(
                    parse_signature_type(Some(s)).unwrap(),
                    Some(SignatureType::Poly1271)
                ),
                "{s} should map to Poly1271"
            );
        }
        assert!(parse_signature_type(Some("nonsense")).is_err());
    }

    #[test]
    fn poly1271_requires_explicit_funder() {
        // A throwaway signer (well-known Anvil key #0) — we only exercise the
        // no-funder derive branch, no network, no real key.
        let signer: PrivKeySigner = LocalSigner::from_str(
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap();
        let creds = PolymarketCredentials {
            funder: None,
            ..Default::default()
        };
        // Poly1271 with no funder → hard error (cannot be derived).
        assert!(resolve_funder(&creds, &signer, Some(SignatureType::Poly1271)).is_err());
        // With an explicit funder it resolves to that address regardless of type.
        let creds_f = PolymarketCredentials {
            funder: Some("0x5aBf4F4Fee1fEc0cd8557dfdEC1b4c9AD5AaDd8D".into()),
            ..Default::default()
        };
        let got = resolve_funder(&creds_f, &signer, Some(SignatureType::Poly1271))
            .unwrap()
            .unwrap();
        assert_eq!(
            got,
            Address::from_str("0x5aBf4F4Fee1fEc0cd8557dfdEC1b4c9AD5AaDd8D").unwrap()
        );
    }
}
