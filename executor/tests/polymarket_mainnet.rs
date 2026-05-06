//! Live Polymarket mainnet tests against the SDK-backed
//! `PolymarketRestClient`. One `#[ignore]`-gated test per `RestOrderClient`
//! method so each can be run manually:
//!
//!   cargo test -p executor --test polymarket_mainnet -- --nocapture <name>
//!
//! Tests that mutate (place / cancel / cancel_all / cancel_market) carry
//! `#[ignore]`. Read-only tests (`get_orders`, `order_status`, `heartbeat`,
//! `test_tokens_by_slug`) run on plain `cargo test`.
//!
//! Credentials: read entirely from `credentials/polymarket.yaml` via
//! `libs::credentials::PolymarketCredentials::load()`. The yaml's
//! `eth_priv_key` is required for the SDK's `authenticate()` step.

use executor::rest::polymarket::PolymarketRestClient;
use executor::rest::RestOrderClient;
use libs::credentials::polymarket::PolymarketCredentials;
use libs::endpoints::polymarket::polymarket as poly_ep;
use libs::endpoints::polymarket::polymarket::gamma;
use libs::protocol::orders::{OrderKind, PlaceOne, Side, Tif};

/// Workspace-relative path to `credentials/polymarket.yaml`.
fn creds_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("credentials/polymarket.yaml")
}

/// Build an authenticated `PolymarketRestClient`. `PolymarketRestClient::new`
/// is async (it makes an HTTP round-trip during the SDK's `authenticate()`
/// step), so this helper is async too. Every test calls it.
async fn build_client() -> PolymarketRestClient {
    let creds = PolymarketCredentials::load(creds_path());
    assert!(
        !creds.address.is_empty(),
        "polymarket address missing from credentials/polymarket.yaml"
    );
    assert!(
        creds.eth_priv_key.as_deref().is_some_and(|s| !s.is_empty()),
        "polymarket eth_priv_key missing from credentials/polymarket.yaml"
    );
    PolymarketRestClient::new(creds, poly_ep::BASE_URL)
        .await
        .expect("construct PolymarketRestClient (this hits Polymarket to derive L2 creds)")
}

// ── Read-only (run on default `cargo test`) ─────────────────────────────────

/// Manual lookup: print yes/no `clobTokenIds` for a Polymarket slug. Edit
/// the `slug` literal to whatever market you want token ids for, then run:
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --nocapture test_tokens_by_slug
#[tokio::test]
async fn test_tokens_by_slug() {
    let slug = "will-bitcoin-dominance-hit-70-before-2027";
    let url = format!("{}{}{}", gamma::BASE_URL, gamma::MARKET_BY_SLUG, slug);

    let body: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .expect("gamma GET")
        .error_for_status()
        .expect("gamma non-2xx")
        .json()
        .await
        .expect("gamma json decode");

    // Gamma encodes `clobTokenIds` as either an array or a JSON string of
    // an array. Outcomes ordering ["Up", "Down"] → index 0 = yes, 1 = no.
    let raw = body
        .get("clobTokenIds")
        .unwrap_or_else(|| panic!("gamma: missing clobTokenIds for slug {slug}"));
    let ids: Vec<String> = match raw {
        serde_json::Value::Array(a) => {
            serde_json::from_value(serde_json::Value::Array(a.clone())).expect("decode array")
        }
        serde_json::Value::String(s) => serde_json::from_str(s).expect("decode string"),
        other => panic!("gamma: clobTokenIds unexpected shape: {other:?}"),
    };

    eprintln!("slug = {slug}");
    eprintln!(
        "yes token id = {}",
        ids.first().map(String::as_str).unwrap_or("<missing>")
    );
    eprintln!(
        "no token id = {}",
        ids.get(1).map(String::as_str).unwrap_or("<missing>")
    );
}

/// `RestOrderClient::heartbeat`. Always returns immediately under the SDK
/// path (no `/v1/heartbeats` round-trip in the current adapter).
#[tokio::test]
async fn heartbeat() {
    let client = build_client().await;
    let ack = client.heartbeat().await.expect("heartbeat should succeed");
    eprintln!("heartbeat: next_due_ms={}", ack.next_due_ms);
}

/// `RestOrderClient::get_orders` → SDK `Client::orders()`.
/// Empty list is the happy path for a fresh wallet; populated is also fine.
#[tokio::test]
async fn get_orders_returns_array() {
    let client = build_client().await;
    let orders = client
        .get_orders()
        .await
        .expect("get_orders should succeed");
    eprintln!("get_orders: {} order(s)", orders.len());
    for ack in &orders {
        eprintln!(
            "  oid={} status={:?} ts_ns={}",
            ack.exchange_oid, ack.status, ack.ts_ns
        );
    }
}

// ── Write paths (run manually with --ignored) ───────────────────────────────

/// `RestOrderClient::place` — places a single GTC limit. Edit the consts at
/// the top of the body for token / side / qty / price, then:
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture place_order
///
/// **Real money on mainnet.** Default px=0.1 is far enough from touch on
/// most up/down 15m markets that it sits in the book without filling, but
/// inspect the orderbook before trusting that.
#[tokio::test]
#[ignore = "places a real order on Polymarket mainnet — run manually"]
async fn place_order() {
    // Edit these to target a different market / size / price.
    let token_id =
        "60105229286427884692660113868141858131134689149752564702347657042086215753173".to_string();
    let side = Side::Buy;
    let qty = 10.0;
    let px = 0.2;

    let client = build_client().await;
    let client_oid = format!(
        "polymarket_mainnet-{}",
        chrono::Utc::now().timestamp_millis()
    );

    let order = PlaceOne {
        client_oid: client_oid.clone(),
        symbol: token_id,
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

/// `RestOrderClient::place_batch` — submits two GTC limits in one call. Edit
/// the array literal in the body to control what gets placed.
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture place_batch_orders
#[tokio::test]
#[ignore = "places real orders on Polymarket mainnet — run manually"]
async fn place_batch_orders() {
    let token_id =
        "60105229286427884692660113868141858131134689149752564702347657042086215753173".to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let orders = vec![
        PlaceOne {
            client_oid: format!("polymarket_mainnet-batch-a-{now_ms}"),
            symbol: token_id.clone(),
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.1,
            qty: 5.0,
            tif: Tif::Gtc,
        },
        PlaceOne {
            client_oid: format!("polymarket_mainnet-batch-b-{now_ms}"),
            symbol: token_id,
            side: Side::Buy,
            kind: OrderKind::Limit,
            px: 0.2,
            qty: 5.0,
            tif: Tif::Gtc,
        },
    ];

    let client = build_client().await;
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

/// `RestOrderClient::cancel` — cancels by exchange-side oid. Edit the const
/// `OID` to whatever you want to cancel (read it from `place_order`'s output
/// or `get_orders_returns_array`).
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture cancel_order
#[tokio::test]
#[ignore = "cancels a real order on Polymarket mainnet — run manually"]
async fn cancel_all_orders_one_by_one() {
    // Replace this with an open exchange_oid before running.
    let client = build_client().await;

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
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture cancel_all_orders
#[tokio::test]
#[ignore = "cancels EVERY open Polymarket order on this account — run manually"]
async fn cancel_all_orders() {
    let client = build_client().await;
    let n = client
        .cancel_all()
        .await
        .expect("cancel_all should succeed");
    eprintln!("cancel_all: cancelled {n} order(s)");
}

/// `RestOrderClient::cancel_market` — cancels every open order on one token.
/// Edit the `TOKEN_ID` const before running.
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture cancel_market_orders
#[tokio::test]
#[ignore = "cancels every order on a single Polymarket token — run manually"]
async fn cancel_market_orders() {
    const TOKEN_ID: &str =
        "60105229286427884692660113868141858131134689149752564702347657042086215753173";

    let client = build_client().await;
    let n = client
        .cancel_market(TOKEN_ID)
        .await
        .expect("cancel_market should succeed");
    eprintln!("cancel_market: cancelled {n} order(s) on token {TOKEN_ID}");
}

/// `RestOrderClient::order_status` — single-order lookup. Edit the const.
///
///   cargo test -p executor --test polymarket_mainnet \
///     -- --ignored --nocapture order_status
#[tokio::test]
#[ignore = "looks up a specific order — edit the OID const before running"]
async fn order_status() {
    let client = build_client().await;
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
