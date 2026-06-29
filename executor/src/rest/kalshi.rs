//! Kalshi v2 REST order client — pure `reqwest`, no SDK.
//!
//! Implements [`RestOrderClient`] against the Kalshi "events orders" REST API
//! (`https://external-api.kalshi.com/trade-api/v2`, see `kalshi.txt` /
//! <https://docs.kalshi.com/api-reference/orders/>). Unlike the SDK-backed
//! Polymarket client, Kalshi auth is local RSA-PSS request signing
//! ([`KalshiCredentials::build_headers`]), so:
//!
//!   - the constructor is **sync** (no `authenticate()` round-trip),
//!   - every request attaches fresh `KALSHI-ACCESS-*` headers.
//!
//! ## Mapping (generic → Kalshi)
//!
//! | generic            | Kalshi                                   |
//! |--------------------|------------------------------------------|
//! | `PlaceOne.symbol`  | `ticker` (e.g. `HIGHNY-24JAN01-T60`)     |
//! | `Side::Buy/Sell`   | `"bid"` / `"ask"`                        |
//! | `px` (f64 dollars) | `price` dollar-string, 4 dp (`"0.5600"`) |
//! | `qty` (f64)        | `count` string, 2 dp (`"10.00"`)         |
//! | `client_oid`       | `client_order_id`                        |
//! | `Tif`              | `time_in_force` token                    |
//!
//! Every order is sent as a **limit** (with `price` + `time_in_force`, both
//! required by the API; no `type` field). `OrderKind::Market` is supported by
//! sending an aggressive limit at the worst in-range price (`0.99` for a buy,
//! `0.01` for a sell — the exact bounds `1.00`/`0.00` are rejected
//! `invalid_price`) with an IOC/FOK tif, so it crosses the book and fills
//! immediately rather than resting.

use async_trait::async_trait;
use chrono::Utc;
use libs::credentials::kalshi::KalshiCredentials;
use libs::endpoints::kalshi::kalshi::endpoints as ep;
use libs::protocol::orders::{
    HeartbeatAck, OrderAck, OrderError, OrderKind, OrderStatus, PlaceOne, Side, Tif,
};
use reqwest::Method;
use rsa::pss::SigningKey;
use rsa::sha2::Sha256;
use serde::{Deserialize, Deserializer, Serialize};

use super::RestOrderClient;
use crate::error::{map_http_status, map_internal, map_reqwest};

pub struct KalshiRestClient {
    http: reqwest::Client,
    /// REST base including the `/trade-api/v2` prefix.
    base_url: String,
    /// Kept for `api_key` + `build_headers`.
    creds: KalshiCredentials,
    /// Parsed once at construction; signing is cheap, parsing is not.
    signing_key: SigningKey<Sha256>,
}

impl KalshiRestClient {
    /// Build a client. **Sync** — Kalshi auth is per-request RSA-PSS signing,
    /// so there is no startup network round-trip (contrast Polymarket). Parses
    /// the PEM secret into a reusable signing key.
    pub fn new(creds: KalshiCredentials, base_url: impl Into<String>) -> anyhow::Result<Self> {
        let signing_key = creds.signing_key()?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            creds,
            signing_key,
        })
    }

    /// Signed request helper. `endpoint` is the path *after* `/trade-api/v2`
    /// (e.g. `/portfolio/events/orders`); it's used both to build the URL and,
    /// via [`ep::sign_path`], to compute the signature path. `query` (e.g.
    /// `"limit=100"`) is appended to the URL but **not** signed — Kalshi's
    /// signature covers the bare path only.
    async fn request<B, R>(
        &self,
        method: Method,
        endpoint: &str,
        query: Option<&str>,
        body: Option<&B>,
    ) -> Result<R, OrderError>
    where
        B: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        let url = match query {
            Some(q) => format!("{}{}?{}", self.base_url, endpoint, q),
            None => format!("{}{}", self.base_url, endpoint),
        };
        let sign_path = ep::sign_path(endpoint);
        let headers = self
            .creds
            .build_headers(&self.signing_key, method.as_str(), &sign_path);

        let mut rb = self.http.request(method, &url);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }

        let resp = rb.send().await.map_err(map_reqwest)?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(map_reqwest)?;
        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&bytes);
            // A 404 with a JSON error body is the API rejecting the request
            // (e.g. wrong path / unknown order), NOT our typed `NotFound` —
            // surface the body so the cause is visible instead of an empty
            // `NotFound`. Only a truly empty 404 stays `NotFound`.
            if status.as_u16() == 404 && !body_str.trim().is_empty() {
                return Err(OrderError::ExchangeRejected {
                    code: Some("404".into()),
                    message: format!("{} (url={})", body_str, url),
                });
            }
            return Err(map_http_status(status, &body_str));
        }
        serde_json::from_slice(&bytes).map_err(map_internal)
    }

    /// Fetch every open order row (one page, `limit=100`). Shared by
    /// `get_orders`, `cancel_all`, and `cancel_market`.
    async fn list_rows(&self) -> Result<Vec<OrderRow>, OrderError> {
        let resp: GetOrdersResp = self
            .request::<(), _>(Method::GET, ep::PORTFOLIO_ORDERS, Some("limit=100"), None)
            .await?;
        Ok(resp.orders)
    }

    /// List the tradable **market** tickers under an event ticker, with each
    /// market's status and (yes) prices. Public endpoint — used to discover the
    /// exact `ticker` an order must reference (the value in a kalshi.com URL is
    /// usually the *event* ticker, not a tradable market ticker). Returns
    /// `(ticker, status, yes_bid, yes_ask)` tuples.
    pub async fn markets_for_event(
        &self,
        event_ticker: &str,
    ) -> Result<Vec<MarketInfo>, OrderError> {
        let query = format!("event_ticker={event_ticker}&limit=200");
        let resp: MarketsResp = self
            .request::<(), _>(Method::GET, ep::MARKETS, Some(&query), None)
            .await?;
        Ok(resp.markets)
    }
}

// ── Mapping helpers ──────────────────────────────────────────────────────────

fn side_to_kalshi(s: Side) -> &'static str {
    match s {
        Side::Buy => "bid",
        Side::Sell => "ask",
    }
}

/// Normalise the `side` Kalshi reports on an order *row* to the `bid`/`ask`
/// vocabulary the create/amend endpoints require. The orders list reports
/// YES/NO contract sides (`"yes"`/`"no"`); amend rejects those with
/// `"side must be bid or ask"`. YES→bid, NO→ask; `bid`/`ask` pass through.
fn order_row_side_to_kalshi(side: &str) -> Option<&'static str> {
    match side.to_ascii_lowercase().as_str() {
        "yes" | "bid" => Some("bid"),
        "no" | "ask" => Some("ask"),
        _ => None,
    }
}

/// Dollar price string, 4 decimals (e.g. `0.56 → "0.5600"`).
fn px_to_dollar_string(px: f64) -> String {
    format!("{px:.4}")
}

/// Contract count string, 2 decimals (e.g. `10.0 → "10.00"`).
fn qty_to_count_string(qty: f64) -> String {
    format!("{qty:.2}")
}

/// Map `Tif` to Kalshi's `time_in_force` token. `Gtd` is rejected — Kalshi
/// expiry uses an `expiration_ts` field, not a tif token, and isn't wired yet.
fn tif_to_kalshi(t: Tif) -> Result<&'static str, OrderError> {
    match t {
        Tif::Gtc => Ok("good_till_canceled"),
        Tif::Ioc => Ok("immediate_or_cancel"),
        Tif::Fok => Ok("fill_or_kill"),
        Tif::Gtd => Err(OrderError::Internal {
            message: "kalshi: Tif::Gtd unsupported (needs expiration_ts; not wired)".into(),
        }),
    }
}

/// `time_in_force` token for a **market** order. Kalshi requires the field but a
/// market order must be marketable, so `Gtc`/`Gtd` (resting) don't apply — map
/// them (and `Ioc`) to `immediate_or_cancel`, and `Fok` to `fill_or_kill`.
fn market_tif_to_kalshi(t: Tif) -> &'static str {
    match t {
        Tif::Fok => "fill_or_kill",
        // Gtc / Gtd / Ioc → IOC: take whatever fills now, cancel the rest.
        _ => "immediate_or_cancel",
    }
}

/// Derive an [`OrderStatus`] from Kalshi fill/remaining/initial counts.
fn derive_status(fill: f64, remaining: f64, initial: f64) -> OrderStatus {
    if remaining <= 0.0 && fill > 0.0 {
        OrderStatus::Filled
    } else if fill > 0.0 && remaining > 0.0 {
        OrderStatus::PartiallyFilled
    } else if remaining > 0.0 {
        OrderStatus::New
    } else if initial > 0.0 {
        // Nothing resting, nothing filled, but it existed → reduced to zero.
        OrderStatus::Canceled
    } else {
        OrderStatus::New
    }
}

fn now_ns() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

/// Build the Kalshi create-order request for one `PlaceOne`. Limit orders carry
/// a dollar price + time-in-force; market orders send `type:"market"` with just
/// the count (Kalshi executes them against resting liquidity immediately, so
/// `px` and `tif` are ignored). Borrows from `p`, so the returned request lives
/// as long as `p`.
fn build_create_req(p: &PlaceOne) -> Result<CreateOrderReq<'_>, OrderError> {
    let side = side_to_kalshi(p.side);
    let count = qty_to_count_string(p.qty);
    match p.kind {
        OrderKind::Limit => Ok(CreateOrderReq::limit(
            &p.symbol,
            &p.client_oid,
            side,
            count,
            px_to_dollar_string(p.px),
            tif_to_kalshi(p.tif)?,
        )),
        OrderKind::Market => Ok(CreateOrderReq::market(
            &p.symbol,
            &p.client_oid,
            side,
            count,
            market_tif_to_kalshi(p.tif),
        )),
    }
}

/// Whether an [`OrderError`] represents a 404 / not-found from Kalshi. Covers
/// both the typed `NotFound` (empty-body 404) and the `ExchangeRejected` with
/// HTTP code `"404"` that `request()` produces for a 404 carrying a JSON body.
fn is_not_found(e: &OrderError) -> bool {
    matches!(e, OrderError::NotFound { .. })
        || matches!(e, OrderError::ExchangeRejected { code: Some(c), .. } if c == "404")
}

/// Deserialize a count that Kalshi sends as either a JSON number or a numeric
/// string (`"10.00"`) into an `f64`. Missing / unparseable → `0.0`.
fn de_count<'de, D>(d: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| D::Error::custom("bad number")),
        serde_json::Value::String(s) => s.parse::<f64>().map_err(D::Error::custom),
        serde_json::Value::Null => Ok(0.0),
        other => Err(D::Error::custom(format!("unexpected count value: {other}"))),
    }
}

// ── Wire structs (mirror kalshi.txt) ─────────────────────────────────────────

/// Kalshi's "events orders" generation requires `self_trade_prevention_type`
/// on every create (a missing field 400s with `missing_parameters`). The doc
/// example uses `taker_at_cross` — the incoming taker crosses against its own
/// resting orders rather than being cancelled — which is the right default for
/// a single-account trader. Make this a const so all create paths agree.
const SELF_TRADE_PREVENTION: &str = "taker_at_cross";

/// Marketable price bounds for a synthetic market order. Kalshi binary
/// contracts trade in the OPEN interval (1¢–99¢): the exact bounds `1.0000` /
/// `0.0000` are rejected `invalid_price`. So a marketable BUY crosses up to
/// `0.99` and a marketable SELL down to `0.01` — still aggressive enough to
/// sweep the whole book at the touch.
const MARKETABLE_BUY_PX: &str = "0.9900";
const MARKETABLE_SELL_PX: &str = "0.0100";

/// Every Kalshi order is sent as a **limit** with an explicit `price` +
/// `time_in_force` (both are required by the events-orders API — omitting either
/// 400s). We don't send a `type` field: a "market" order is just an aggressive
/// limit at the worst-acceptable price (see [`CreateOrderReq::market`]), which
/// crosses the book and fills immediately. Avoiding `type` also sidesteps the
/// API's per-type field validation.
#[derive(Serialize)]
struct CreateOrderReq<'a> {
    ticker: &'a str,
    client_order_id: &'a str,
    side: &'a str, // "bid" | "ask"
    count: String, // "10.00"
    price: String, // "0.5600"
    time_in_force: &'a str,
    /// Required by the events-orders API (see [`SELF_TRADE_PREVENTION`]).
    self_trade_prevention_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_only: Option<bool>,
}

impl<'a> CreateOrderReq<'a> {
    /// Build a limit order: the caller's price + time_in_force.
    fn limit(
        ticker: &'a str,
        client_order_id: &'a str,
        side: &'a str,
        count: String,
        price: String,
        time_in_force: &'a str,
    ) -> Self {
        Self {
            ticker,
            client_order_id,
            side,
            count,
            price,
            time_in_force,
            self_trade_prevention_type: SELF_TRADE_PREVENTION,
            post_only: None,
        }
    }

    /// Build a market order as an aggressive limit at the *worst-acceptable*
    /// price that guarantees a cross: a `bid` (buy) pays up to `0.99`, an `ask`
    /// (sell) sells down to `0.01` (the in-range extremes — see
    /// [`MARKETABLE_BUY_PX`]). Paired with an IOC/FOK `time_in_force` it fills
    /// against resting liquidity at the touch and never rests at this bound.
    fn market(
        ticker: &'a str,
        client_order_id: &'a str,
        side: &'a str,
        count: String,
        time_in_force: &'a str,
    ) -> Self {
        let price = if side == "bid" {
            MARKETABLE_BUY_PX
        } else {
            MARKETABLE_SELL_PX
        };
        Self::limit(
            ticker,
            client_order_id,
            side,
            count,
            price.to_string(),
            time_in_force,
        )
    }
}

#[derive(Deserialize)]
struct CreateOrderResp {
    order_id: String,
    // `client_order_id` is echoed by Kalshi but we already hold the caller's
    // value, so it's not deserialized here (serde ignores unknown fields).
    #[serde(default, deserialize_with = "de_count")]
    fill_count: f64,
    #[serde(default, deserialize_with = "de_count")]
    remaining_count: f64,
}

#[derive(Serialize)]
struct BatchCreateReq<'a> {
    orders: Vec<CreateOrderReq<'a>>,
}

#[derive(Deserialize)]
struct BatchCreateResp {
    #[serde(default)]
    orders: Vec<CreateOrderResp>,
}

#[derive(Serialize)]
struct AmendReq<'a> {
    ticker: &'a str,
    side: &'a str,
    price: String,
    count: String,
    client_order_id: &'a str,
    updated_client_order_id: &'a str,
}

#[derive(Deserialize)]
struct AmendResp {
    order_id: String,
    #[serde(default)]
    client_order_id: String,
    #[serde(default, deserialize_with = "de_count")]
    fill_count: f64,
    #[serde(default, deserialize_with = "de_count")]
    remaining_count: f64,
}

#[derive(Deserialize)]
struct CancelResp {
    #[serde(default)]
    order: Option<OrderRow>,
}

#[derive(Deserialize)]
struct SingleOrderResp {
    order: OrderRow,
}

#[derive(Deserialize)]
struct GetOrdersResp {
    #[serde(default)]
    orders: Vec<OrderRow>,
}

#[derive(Deserialize)]
struct MarketsResp {
    #[serde(default)]
    markets: Vec<MarketInfo>,
}

/// Public market metadata — enough to pick a tradable `ticker`. Prices are in
/// cents on the public markets endpoint (0–100); kept as-is for display.
#[derive(Debug, Deserialize)]
pub struct MarketInfo {
    pub ticker: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub yes_bid: Option<i64>,
    #[serde(default)]
    pub yes_ask: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
}

/// A row from the orders list / single-order envelope. Kalshi names the count
/// fields with a `_fp` ("fixed point") suffix here.
#[derive(Deserialize)]
struct OrderRow {
    order_id: String,
    #[serde(default)]
    client_order_id: String,
    #[serde(default)]
    ticker: String,
    /// Kalshi sends YES/NO sides; we surface `bid`/`ask` semantics via the
    /// `side` string when present (used to reconstruct an amend).
    #[serde(default)]
    side: String,
    #[serde(default)]
    yes_price_dollars: Option<String>,
    #[serde(default)]
    no_price_dollars: Option<String>,
    #[serde(default, deserialize_with = "de_count")]
    fill_count_fp: f64,
    #[serde(default, deserialize_with = "de_count")]
    remaining_count_fp: f64,
    #[serde(default, deserialize_with = "de_count")]
    initial_count_fp: f64,
}

impl OrderRow {
    fn status(&self) -> OrderStatus {
        derive_status(
            self.fill_count_fp,
            self.remaining_count_fp,
            self.initial_count_fp,
        )
    }
}

// ── Trait impl ───────────────────────────────────────────────────────────────

#[async_trait]
impl RestOrderClient for KalshiRestClient {
    async fn place(&self, p: &PlaceOne) -> Result<OrderAck, OrderError> {
        let req = build_create_req(p)?;
        let resp: CreateOrderResp = self
            .request(Method::POST, ep::EVENTS_ORDERS, None, Some(&req))
            .await?;
        Ok(OrderAck {
            client_oid: p.client_oid.clone(),
            exchange_oid: resp.order_id,
            status: derive_status(
                resp.fill_count,
                resp.remaining_count,
                resp.fill_count + resp.remaining_count,
            ),
            ex_timestamp: 0,
            recv_timestamp: now_ns(),
            fill: None,
        })
    }

    async fn place_batch(
        &self,
        os: &[PlaceOne],
    ) -> Result<Vec<Result<OrderAck, OrderError>>, OrderError> {
        // Validate + build every leg up front. A leg that fails to build
        // (e.g. `Tif::Gtd`) never reaches the wire; it's slotted back into the
        // result by index so callers keep a 1:1 mapping.
        let mut reqs: Vec<CreateOrderReq> = Vec::with_capacity(os.len());
        // index in `os` → index in `reqs` (None for legs that failed validation)
        let mut slot: Vec<Option<usize>> = Vec::with_capacity(os.len());
        let mut errs: Vec<Option<OrderError>> = Vec::with_capacity(os.len());

        for p in os {
            match build_create_req(p) {
                Ok(req) => {
                    slot.push(Some(reqs.len()));
                    errs.push(None);
                    reqs.push(req);
                }
                Err(e) => {
                    slot.push(None);
                    errs.push(Some(e));
                }
            }
        }

        // Map each submitted leg's response by position.
        let mut responses: Vec<Result<CreateOrderResp, OrderError>> = Vec::new();
        if !reqs.is_empty() {
            let body = BatchCreateReq { orders: reqs };
            let resp: BatchCreateResp = self
                .request(Method::POST, ep::EVENTS_ORDERS_BATCHED, None, Some(&body))
                .await?;
            // Positional zip; if the venue returns fewer rows than sent, the
            // missing tail becomes ExchangeRejected.
            let n = body.orders.len();
            let mut it = resp.orders.into_iter();
            for _ in 0..n {
                match it.next() {
                    Some(r) => responses.push(Ok(r)),
                    None => responses.push(Err(OrderError::ExchangeRejected {
                        code: None,
                        message: "kalshi batch: missing response row for submitted order".into(),
                    })),
                }
            }
        }

        // Reassemble in original order.
        let mut out = Vec::with_capacity(os.len());
        for (i, p) in os.iter().enumerate() {
            if let Some(e) = errs[i].take() {
                out.push(Err(e));
                continue;
            }
            let ri = slot[i].expect("validated leg has a request slot");
            out.push(match &responses[ri] {
                Ok(r) => Ok(OrderAck {
                    client_oid: p.client_oid.clone(),
                    exchange_oid: r.order_id.clone(),
                    status: derive_status(
                        r.fill_count,
                        r.remaining_count,
                        r.fill_count + r.remaining_count,
                    ),
                    ex_timestamp: 0,
                    recv_timestamp: now_ns(),
                    fill: None,
                }),
                Err(e) => Err(e.clone()),
            });
        }
        Ok(out)
    }

    async fn update(
        &self,
        client_oid: &str,
        exchange_oid: &str,
        new_px: Option<f64>,
        new_qty: Option<f64>,
    ) -> Result<OrderAck, OrderError> {
        // Amend needs ticker + side + full price + full count, none of which
        // are in the call args. Recover them from the live order first, then
        // overlay the requested changes.
        let path = format!("{}{}", ep::ORDER_BY_ID, exchange_oid);
        let cur: SingleOrderResp = self
            .request::<(), _>(Method::GET, &path, None, None)
            .await?;
        let row = cur.order;

        // Amend wants `bid`/`ask`; the orders list reports `yes`/`no`.
        let side = order_row_side_to_kalshi(&row.side);
        if row.ticker.is_empty() || side.is_none() {
            return Err(OrderError::Internal {
                message: format!(
                    "kalshi amend: could not recover ticker/side from live order \
                     (ticker={:?}, side={:?})",
                    row.ticker, row.side
                ),
            });
        }
        let side = side.expect("checked is_none above");

        // Current price: prefer the side that matches the order. A `yes`/`bid`
        // order is priced by `yes_price_dollars`; a `no`/`ask` order by
        // `no_price_dollars`.
        let cur_px = match side {
            "bid" => row.yes_price_dollars.as_deref(),
            _ => row.no_price_dollars.as_deref(),
        }
        .or(row.yes_price_dollars.as_deref())
        .or(row.no_price_dollars.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
        let cur_count = row.remaining_count_fp + row.fill_count_fp;

        let price = px_to_dollar_string(new_px.unwrap_or(cur_px));
        let count = qty_to_count_string(new_qty.unwrap_or(cur_count));

        let amend_path = format!(
            "{}{}{}",
            ep::EVENTS_ORDER_BY_ID,
            exchange_oid,
            ep::AMEND_SUFFIX
        );
        let req = AmendReq {
            ticker: &row.ticker,
            side,
            price,
            count,
            client_order_id: client_oid,
            updated_client_order_id: client_oid,
        };
        let resp: AmendResp = self
            .request(Method::POST, &amend_path, None, Some(&req))
            .await?;
        Ok(OrderAck {
            client_oid: if resp.client_order_id.is_empty() {
                client_oid.to_string()
            } else {
                resp.client_order_id
            },
            exchange_oid: resp.order_id,
            status: derive_status(
                resp.fill_count,
                resp.remaining_count,
                resp.fill_count + resp.remaining_count,
            ),
            ex_timestamp: 0,
            recv_timestamp: now_ns(),
            fill: None,
        })
    }

    async fn cancel(&self, exchange_oid: &str) -> Result<OrderAck, OrderError> {
        let path = format!("{}{}", ep::ORDER_BY_ID, exchange_oid);
        let result = self
            .request::<(), CancelResp>(Method::DELETE, &path, None, None)
            .await;

        let client_oid = match result {
            Ok(resp) => resp.order.map(|o| o.client_order_id).unwrap_or_default(),
            // Cancel is idempotent: a 404 means the order is already gone
            // (filled / expired / cancelled in a prior pass), which is exactly
            // the end state cancel wants. Treat it as success rather than
            // erroring — this keeps `cancel_all` sweeps and retries clean.
            Err(e) if is_not_found(&e) => String::new(),
            Err(e) => return Err(e),
        };

        Ok(OrderAck {
            client_oid,
            exchange_oid: exchange_oid.to_string(),
            status: OrderStatus::Canceled,
            ex_timestamp: 0,
            recv_timestamp: now_ns(),
            fill: None,
        })
    }

    async fn cancel_all(&self) -> Result<u32, OrderError> {
        // No bulk endpoint — list then DELETE each. Per-order failures are
        // swallowed so one stale id can't abort the sweep; we count successes.
        let rows = self.list_rows().await?;
        let mut cancelled = 0u32;
        for row in rows {
            if self.cancel(&row.order_id).await.is_ok() {
                cancelled += 1;
            }
        }
        Ok(cancelled)
    }

    async fn cancel_market(&self, symbol: &str) -> Result<u32, OrderError> {
        let rows = self.list_rows().await?;
        let mut cancelled = 0u32;
        for row in rows.into_iter().filter(|r| r.ticker == symbol) {
            if self.cancel(&row.order_id).await.is_ok() {
                cancelled += 1;
            }
        }
        Ok(cancelled)
    }

    async fn get_order(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError> {
        self.order_status(exchange_oid).await
    }

    async fn get_orders(&self) -> Result<Vec<OrderAck>, OrderError> {
        let rows = self.list_rows().await?;
        Ok(rows
            .into_iter()
            .map(|row| OrderAck {
                client_oid: row.client_order_id.clone(),
                status: row.status(),
                exchange_oid: row.order_id,
                ex_timestamp: 0,
                recv_timestamp: now_ns(),
                fill: None,
            })
            .collect())
    }

    async fn order_status(&self, exchange_oid: &str) -> Result<OrderStatus, OrderError> {
        let path = format!("{}{}", ep::ORDER_BY_ID, exchange_oid);
        let resp: SingleOrderResp = self
            .request::<(), _>(Method::GET, &path, None, None)
            .await?;
        Ok(resp.order.status())
    }

    async fn heartbeat(&self) -> Result<HeartbeatAck, OrderError> {
        // Kalshi's REST API is stateless (every request is RSA-PSS signed) —
        // there is no heartbeat / keepalive to send, so this is a no-op static
        // ack (matching the Polymarket client). `next_due_ms = MAX` means
        // "always alive; never schedule a follow-up". For an actual
        // connectivity/credential probe use `get_orders`.
        Ok(HeartbeatAck {
            next_due_ms: u64::MAX,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_and_format_mapping() {
        assert_eq!(side_to_kalshi(Side::Buy), "bid");
        assert_eq!(side_to_kalshi(Side::Sell), "ask");
        assert_eq!(px_to_dollar_string(0.56), "0.5600");
        assert_eq!(px_to_dollar_string(0.5), "0.5000");
        assert_eq!(qty_to_count_string(10.0), "10.00");
        assert_eq!(qty_to_count_string(5.5), "5.50");
    }

    #[test]
    fn order_row_side_normalises_yes_no_to_bid_ask() {
        assert_eq!(order_row_side_to_kalshi("yes"), Some("bid"));
        assert_eq!(order_row_side_to_kalshi("no"), Some("ask"));
        assert_eq!(order_row_side_to_kalshi("YES"), Some("bid"));
        assert_eq!(order_row_side_to_kalshi("bid"), Some("bid"));
        assert_eq!(order_row_side_to_kalshi("ask"), Some("ask"));
        assert_eq!(order_row_side_to_kalshi(""), None);
        assert_eq!(order_row_side_to_kalshi("weird"), None);
    }

    #[test]
    fn is_not_found_covers_both_404_shapes() {
        assert!(is_not_found(&OrderError::NotFound {
            exchange_oid: String::new()
        }));
        assert!(is_not_found(&OrderError::ExchangeRejected {
            code: Some("404".into()),
            message: "not found".into()
        }));
        assert!(!is_not_found(&OrderError::ExchangeRejected {
            code: Some("400".into()),
            message: "bad".into()
        }));
        assert!(!is_not_found(&OrderError::Timeout));
    }

    #[test]
    fn tif_mapping() {
        assert_eq!(tif_to_kalshi(Tif::Gtc).unwrap(), "good_till_canceled");
        assert_eq!(tif_to_kalshi(Tif::Ioc).unwrap(), "immediate_or_cancel");
        assert_eq!(tif_to_kalshi(Tif::Fok).unwrap(), "fill_or_kill");
        assert!(tif_to_kalshi(Tif::Gtd).is_err());
    }

    #[test]
    fn status_derivation() {
        // fresh resting order
        assert_eq!(derive_status(0.0, 10.0, 10.0), OrderStatus::New);
        // partial
        assert_eq!(derive_status(4.0, 6.0, 10.0), OrderStatus::PartiallyFilled);
        // fully filled
        assert_eq!(derive_status(10.0, 0.0, 10.0), OrderStatus::Filled);
        // reduced to zero with no fills → canceled
        assert_eq!(derive_status(0.0, 0.0, 10.0), OrderStatus::Canceled);
    }

    #[test]
    fn de_count_accepts_string_or_number() {
        #[derive(Deserialize)]
        struct W {
            #[serde(deserialize_with = "de_count")]
            c: f64,
        }
        let a: W = serde_json::from_str(r#"{"c":"10.00"}"#).unwrap();
        assert_eq!(a.c, 10.0);
        let b: W = serde_json::from_str(r#"{"c":7}"#).unwrap();
        assert_eq!(b.c, 7.0);
        let n: W = serde_json::from_str(r#"{"c":null}"#).unwrap();
        assert_eq!(n.c, 0.0);
    }

    #[test]
    fn limit_order_req_serializes_to_kalshi_shape() {
        let p = PlaceOne {
            client_oid: "abc".into(),
            symbol: "HIGHNY-24JAN01-T60".into(),
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.56,
            qty: 10.0,
            tif: Tif::Gtc,
        };
        let req = build_create_req(&p).unwrap();
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["ticker"], "HIGHNY-24JAN01-T60");
        // No `type` field — every order is a limit on the wire.
        assert!(v.get("type").is_none());
        assert_eq!(v["side"], "bid");
        assert_eq!(v["count"], "10.00");
        assert_eq!(v["price"], "0.5600");
        assert_eq!(v["time_in_force"], "good_till_canceled");
        assert_eq!(v["self_trade_prevention_type"], "taker_at_cross");
        assert!(v.get("post_only").is_none());
    }

    #[test]
    fn market_order_req_uses_marketable_price_and_tif() {
        // SELL → ask → worst-acceptable price 0.0000 (crosses down the book).
        let sell = PlaceOne {
            client_oid: "abc".into(),
            symbol: "HIGHNY-24JAN01-T60".into(),
            side: Side::Sell,
            kind: OrderKind::Market,
            px: 0.0,
            qty: 7.0,
            tif: Tif::Ioc,
        };
        let v: serde_json::Value = serde_json::to_value(&build_create_req(&sell).unwrap()).unwrap();
        // No `type` field — a "market" order is an aggressive limit on the wire.
        assert!(v.get("type").is_none());
        assert_eq!(v["side"], "ask");
        assert_eq!(v["count"], "7.00");
        // Kalshi REQUIRES both price and time_in_force on every order; a market
        // SELL crosses down to the in-range floor 0.0100 (0.0000 is rejected).
        assert_eq!(v["price"], "0.0100");
        assert_eq!(v["time_in_force"], "immediate_or_cancel");
        assert_eq!(v["self_trade_prevention_type"], "taker_at_cross");

        // BUY → bid → in-range ceiling 0.9900 (1.0000 is rejected invalid_price).
        let buy = PlaceOne {
            kind: OrderKind::Market,
            side: Side::Buy,
            ..sell
        };
        let vb: serde_json::Value = serde_json::to_value(&build_create_req(&buy).unwrap()).unwrap();
        assert_eq!(vb["side"], "bid");
        assert_eq!(vb["price"], "0.9900");
    }

    #[test]
    fn market_tif_maps_to_marketable_tokens() {
        // Fok → fill_or_kill; everything else (incl. resting Gtc/Gtd) → IOC.
        assert_eq!(market_tif_to_kalshi(Tif::Fok), "fill_or_kill");
        assert_eq!(market_tif_to_kalshi(Tif::Ioc), "immediate_or_cancel");
        assert_eq!(market_tif_to_kalshi(Tif::Gtc), "immediate_or_cancel");
        assert_eq!(market_tif_to_kalshi(Tif::Gtd), "immediate_or_cancel");
    }

    #[test]
    fn create_order_resp_parses_string_counts() {
        let resp: CreateOrderResp = serde_json::from_str(
            r#"{"order_id":"o1","client_order_id":"c1","fill_count":"0.00","remaining_count":"10.00","ts_ms":1}"#,
        )
        .unwrap();
        assert_eq!(resp.order_id, "o1");
        assert_eq!(resp.fill_count, 0.0);
        assert_eq!(resp.remaining_count, 10.0);
    }
}
