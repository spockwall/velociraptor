//! Live Kalshi tests against `executor::rest::kalshi::KalshiRestClient`.
//! One `#[ignore]`-gated test per write method so each can be run manually:
//!
//!   cargo test -p executor --test kalshi_mainnet -- --nocapture <name>
//!
//! Tests that mutate (place / place_batch / update / cancel / cancel_all /
//! cancel_market) carry `#[ignore]`. Read-only tests (`get_orders`,
//! `order_status`, `heartbeat`) run on plain `cargo test`.
//!
//! Credentials: read from `credentials/kalshi.yaml` via
//! `libs::credentials::kalshi::KalshiCredentials::load()`. Auth is local
//! RSA-PSS signing — `KalshiRestClient::new` is sync (no network round-trip).
//!
//! NOTE: this targets the **production** external-api host (real money). The
//! `place_*` defaults sit far from touch but inspect the market first.

use executor::rest::kalshi::KalshiRestClient;
use executor::rest::RestOrderClient;
use libs::credentials::kalshi::KalshiCredentials;
use libs::endpoints::kalshi::kalshi as kalshi_ep;
use libs::protocol::orders::{OrderKind, PlaceOne, Side, Tif};

// A tradable **market** ticker (one outcome), NOT the event ticker
// `KXMENWORLDCUP-26`. Discover valid tickers with the `markets_for_event` test.
// Must be UPPERCASE and reference a single market, else Kalshi 404s
// `market_not_found`.
const MARKET_TICKER: &str = "KXMENWORLDCUP-26-AR";

/// Workspace-relative path to `credentials/kalshi.yaml`.
fn creds_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("credentials/kalshi.yaml")
}

/// Build a `KalshiRestClient`. Sync — RSA-PSS auth is local, no `authenticate()`.
fn build_client() -> KalshiRestClient {
    let creds = KalshiCredentials::load(creds_path());
    assert!(
        !creds.api_key.is_empty(),
        "kalshi api_key missing from credentials/kalshi.yaml"
    );
    assert!(
        !creds.secret.is_empty(),
        "kalshi secret missing from credentials/kalshi.yaml"
    );
    KalshiRestClient::new(creds, kalshi_ep::EXTERNAL_API_BASE_URL)
        .expect("construct KalshiRestClient (local RSA-PSS, no network)")
}

// ── Read-only (run on default `cargo test`) ─────────────────────────────────

/// `RestOrderClient::heartbeat` → GET `/exchange/heartbeat`.
#[tokio::test]
async fn heartbeat() {
    let client = build_client();
    let ack = client.heartbeat().await.expect("heartbeat should succeed");
    eprintln!("heartbeat: next_due_ms={}", ack.next_due_ms);
}

/// `RestOrderClient::get_orders` → GET `/portfolio/orders?limit=100`.
/// Empty list is the happy path for a fresh account; populated is also fine.
#[tokio::test]
async fn get_orders_returns_array() {
    let client = build_client();
    let orders = client
        .get_orders()
        .await
        .expect("get_orders should succeed");
    eprintln!("get_orders: {} order(s)", orders.len());
    for ack in &orders {
        eprintln!(
            "  oid={} client_oid={} status={:?} ts_ns={}",
            ack.exchange_oid, ack.client_oid, ack.status, ack.ts_ns
        );
    }
}

/// Discover the tradable **market** tickers under an event. Kalshi.com URLs
/// usually carry the *event* ticker (e.g. `KXMENWORLDCUP-26`), but an order
/// must reference a market ticker (one per outcome, e.g.
/// `KXMENWORLDCUP-26-ARG`). Edit `EVENT` (UPPERCASE event ticker) and run to
/// print every market + status/prices, then copy a real ticker into
/// `MARKET_TICKER` above:
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --nocapture markets_for_event
#[tokio::test]
async fn markets_for_event() {
    const EVENT: &str = "KXMENWORLDCUP-26";

    let client = build_client();
    let markets = client
        .markets_for_event(EVENT)
        .await
        .expect("markets_for_event should succeed");
    eprintln!("event {EVENT}: {} market(s)", markets.len());
    for m in &markets {
        eprintln!(
            "  ticker={} status={} yes_bid={:?} yes_ask={:?} title={:?}",
            m.ticker, m.status, m.yes_bid, m.yes_ask, m.title
        );
    }
}

// ── Write paths (run manually with --ignored) ───────────────────────────────

/// `RestOrderClient::place` — places a single GTC limit. Edit the consts at
/// the top of the body for ticker / side / qty / price, then:
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture place_order
///
/// **Real money on the production host.** Inspect the market's book first; the
/// default px=0.05 is intended to rest without filling but verify.
#[tokio::test]
#[ignore = "places a real order on Kalshi — run manually"]
async fn place_order() {
    // Edit these to target a different market / size / price.
    let ticker = MARKET_TICKER.to_string();
    let side = Side::Buy;
    let qty = 10.0; // contracts
    let px = 0.05; // dollars

    let client = build_client();
    let client_oid = format!("kalshi-mainnet-{}", chrono::Utc::now().timestamp_millis());

    let order = PlaceOne {
        client_oid: client_oid.clone(),
        symbol: ticker,
        side,
        kind: OrderKind::Limit,
        px,
        qty,
        tif: Tif::Gtc,
    };

    eprintln!("place: {order:?}");
    let ack = client.place(&order).await.expect("place should succeed");
    eprintln!(
        "place: ACK exchange_oid={} status={:?} client_oid={}",
        ack.exchange_oid, ack.status, ack.client_oid
    );
}

/// `RestOrderClient::place_batch` — submits two GTC limits via the batched
/// endpoint. Edit the array literal in the body to control what gets placed.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture place_batch_orders
#[tokio::test]
#[ignore = "places real orders on Kalshi — run manually"]
async fn place_batch_orders() {
    let ticker = MARKET_TICKER.to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let orders = vec![
        PlaceOne {
            client_oid: format!("kalshi-mainnet-batch-a-{now_ms}"),
            symbol: ticker.clone(),
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.05,
            qty: 5.0,
            tif: Tif::Gtc,
        },
        PlaceOne {
            client_oid: format!("kalshi-mainnet-batch-b-{now_ms}"),
            symbol: ticker,
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.06,
            qty: 5.0,
            tif: Tif::Gtc,
        },
    ];

    let client = build_client();
    eprintln!("place_batch: {} order(s)", orders.len());
    let acks = client
        .place_batch(&orders)
        .await
        .expect("place_batch should succeed");
    for (i, r) in acks.iter().enumerate() {
        match r {
            Ok(a) => eprintln!(
                "  [{i}] ok exchange_oid={} status={:?}",
                a.exchange_oid, a.status
            ),
            Err(e) => eprintln!("  [{i}] err {e:?}"),
        }
    }
}

/// `RestOrderClient::update` — amends an existing order's price/qty. Places a
/// fresh order, then amends it. Real money.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture update_order
#[tokio::test]
#[ignore = "places then amends a real order on Kalshi — run manually"]
async fn update_order() {
    let ticker = MARKET_TICKER.to_string();
    let client = build_client();
    let client_oid = format!("kalshi-mainnet-amend-{}", chrono::Utc::now().timestamp_millis());

    let order = PlaceOne {
        client_oid: client_oid.clone(),
        symbol: ticker,
        side: Side::Buy,
        kind: OrderKind::Limit,
        px: 0.05,
        qty: 10.0,
        tif: Tif::Gtc,
    };
    let ack = client.place(&order).await.expect("place should succeed");
    eprintln!("place: oid={} status={:?}", ack.exchange_oid, ack.status);

    // Amend: bump price, shrink qty.
    let amended = client
        .update(&client_oid, &ack.exchange_oid, Some(0.06), Some(8.0))
        .await
        .expect("update should succeed");
    eprintln!(
        "update: oid={} status={:?} client_oid={}",
        amended.exchange_oid, amended.status, amended.client_oid
    );
}

/// `RestOrderClient::cancel` — cancels every open order one at a time.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture cancel_all_orders_one_by_one
#[tokio::test]
#[ignore = "cancels real Kalshi orders one by one — run manually"]
async fn cancel_all_orders_one_by_one() {
    let client = build_client();
    let orders = client
        .get_orders()
        .await
        .expect("get_orders should succeed");
    eprintln!("get_orders: {} order(s)", orders.len());

    for ack in &orders {
        let res = client
            .cancel(ack.exchange_oid.as_str())
            .await
            .expect("cancel should succeed");
        eprintln!(
            "cancel: ACK exchange_oid={} status={:?}",
            res.exchange_oid, res.status
        );
    }
}

/// `RestOrderClient::cancel_all` — wipes every open order on the account.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture cancel_all_orders
#[tokio::test]
#[ignore = "cancels EVERY open Kalshi order on this account — run manually"]
async fn cancel_all_orders() {
    let client = build_client();
    let n = client
        .cancel_all()
        .await
        .expect("cancel_all should succeed");
    eprintln!("cancel_all: cancelled {n} order(s)");
}

/// `RestOrderClient::cancel_market` — cancels every open order on one ticker.
/// Edit the `TICKER` const before running.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture cancel_market_orders
#[tokio::test]
#[ignore = "cancels every order on a single Kalshi ticker — run manually"]
async fn cancel_market_orders() {

    let client = build_client();
    let n = client
        .cancel_market(MARKET_TICKER)
        .await
        .expect("cancel_market should succeed");
    eprintln!("cancel_market: cancelled {n} order(s) on ticker {MARKET_TICKER}");
}

/// `RestOrderClient::order_status` — single-order lookup for every open order.
///
///   cargo test -p executor --test kalshi_mainnet \
///     -- --ignored --nocapture order_status
#[tokio::test]
#[ignore = "looks up status for each open order — run manually"]
async fn order_status() {
    let client = build_client();
    let orders = client
        .get_orders()
        .await
        .expect("get_orders should succeed");
    eprintln!("get_orders: {} order(s)", orders.len());

    for ack in &orders {
        let status = client
            .order_status(ack.exchange_oid.as_str())
            .await
            .expect("order_status should succeed");
        eprintln!("order_status: oid={} status={status:?}", ack.exchange_oid);
    }
}
