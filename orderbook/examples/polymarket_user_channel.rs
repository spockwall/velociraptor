//! Polymarket private user-channel verification with rolling-window rotation.
//!
//! Connects to `wss://ws-subscriptions-clob.polymarket.com/ws/user` with the
//! L2 credentials from `credentials/polymarket.yaml`, subscribes to every
//! enabled market in `configs/polymarket.yaml`, and pretty-prints every
//! incoming message to stderr.
//!
//! Each market runs under the shared rolling-window scheduler: a per-window
//! task connects to the user channel with that window's `conditionId`, and
//! rotates cleanly at window boundaries — same pattern as
//! `polymarket_orderbook.rs`.
//!
//! Bypasses `OrderbookSystem` / `OrderbookEngine` — the engine's `start`
//! loop only matches `OrderbookMessage::OrderbookUpdate`, so `UserEvent`
//! variants would be silently dropped. This example spawns
//! `PolymarketConnection` directly against a private mpsc receiver.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example polymarket_user_channel
//! ```

use anyhow::Result;
use libs::configs::PolymarketFileConfig;
use libs::credentials::PolymarketCredentials;
use libs::protocol::{ExchangeName, UserEvent};
use orderbook::connection::{ConnectionConfig, ConnectionTrait, SystemControl};
use orderbook::exchanges::polymarket::{
    WindowTask, build_client, fetch_condition_id, run_rolling_scheduler,
};
use orderbook::types::endpoints::polymarket;
use orderbook::types::orderbook::OrderbookMessage;
use orderbook::{PolymarketConnection, PolymarketSubMsgBuilder};
use tokio::sync::mpsc;

const CREDENTIALS_PATH: &str = "credentials/polymarket.yaml";
const CONFIG_PATH: &str = "configs/polymarket.yaml";

// ── Per-window user-channel task ──────────────────────────────────────────────

/// Spawn a user-channel connection for one window (`full_slug`).
///
/// Resolves the market's `conditionId` via the Gamma API, builds the signed
/// subscribe frame, spawns `PolymarketConnection::run()`, and drains the
/// private mpsc on a printer task. Returns a `WindowTask` that the scheduler
/// will stop at the window boundary.
async fn spawn_user_channel_task(
    base_slug: String,
    full_slug: String,
    creds: PolymarketCredentials,
) -> Option<WindowTask> {
    // Resolve conditionId off the hot path.
    let slug_for_fetch = full_slug.clone();
    let condition_id = tokio::task::spawn_blocking(move || {
        let client = build_client()?;
        fetch_condition_id(&client, &slug_for_fetch)
    })
    .await
    .ok()
    .flatten();

    let Some(condition_id) = condition_id else {
        eprintln!("[user-task] {full_slug}: no conditionId yet (market not open?); will retry");
        return None;
    };

    eprintln!("[user-task] {full_slug} → condition {condition_id}");

    let passphrase = creds.passphrase.clone().unwrap_or_default();
    let sub_json = PolymarketSubMsgBuilder::new()
        .with_auth(&creds.api_key, &creds.secret, &passphrase)
        .with_condition(&condition_id)
        .build();

    let cfg = ConnectionConfig::new(ExchangeName::Polymarket)
        .set_ws_url(polymarket::ws::USER_STREAM)
        .set_subscription_message(sub_json)
        .set_api_credentials(
            creds.api_key.clone(),
            creds.secret.clone(),
            creds.passphrase.clone(),
        );

    let (tx, mut rx) = mpsc::unbounded_channel::<OrderbookMessage>();
    let control = SystemControl::new();

    let mut conn = PolymarketConnection::new(cfg, tx, control.clone());
    let conn_ctrl = control.clone();
    let conn_label = full_slug.clone();
    let conn_task = tokio::spawn(async move {
        if let Err(e) = conn.run().await {
            eprintln!("[user-task] {conn_label} conn exited: {e:?}");
        }
        conn_ctrl.shutdown();
    });

    // Printer task — prints until the connection closes the channel (scheduler
    // aborts the conn task at window boundary, which drops tx).
    let print_ctrl = control.clone();
    let label_for_print = base_slug.clone();
    let full_for_print = full_slug.clone();
    let printer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if print_ctrl.is_shutdown() {
                break;
            }
            print_msg(&label_for_print, &full_for_print, &msg);
        }
    });

    // Combined handle: when the scheduler aborts this, both inner tasks are
    // no longer polled — the conn task also drops tx, closing rx in printer.
    let handle = tokio::spawn(async move {
        let _ = conn_task.await;
        printer.abort();
    });

    Some(WindowTask::new(full_slug, control, handle))
}

// ── Pretty printer ────────────────────────────────────────────────────────────

fn print_msg(base: &str, full: &str, msg: &OrderbookMessage) {
    let tag = format!("{base}@{full}");
    match msg {
        OrderbookMessage::UserEvent(UserEvent::OrderUpdate {
            client_oid,
            exchange_oid,
            symbol,
            side,
            px,
            qty,
            filled,
            status,
            ..
        }) => {
            eprintln!(
                "[{tag}] ORDER {status:?} {side:?} px={px:.4} qty={qty:.4} filled={filled:.4} oid={exchange_oid} owner={client_oid} asset={symbol}"
            );
        }
        OrderbookMessage::UserEvent(UserEvent::Fill {
            exchange_oid,
            client_oid,
            symbol,
            side,
            px,
            qty,
            ..
        }) => {
            eprintln!(
                "[{tag}] FILL {side:?} px={px:.4} qty={qty:.4} trade={exchange_oid} taker={client_oid} asset={symbol}"
            );
        }
        OrderbookMessage::UserEvent(UserEvent::Balance {
            asset,
            free,
            locked,
            ..
        }) => {
            eprintln!("[{tag}] BALANCE {asset} free={free} locked={locked}");
        }
        OrderbookMessage::UserEvent(UserEvent::Position {
            symbol,
            size,
            avg_px,
            ..
        }) => {
            eprintln!("[{tag}] POSITION {symbol} size={size} avg_px={avg_px}");
        }
        OrderbookMessage::OrderbookUpdate(u) => {
            eprintln!(
                "[{tag}] BOOK {} {:?} n={}",
                u.symbol,
                u.action,
                u.orders.len()
            );
        }
        OrderbookMessage::Base(b) => {
            eprintln!("[{tag}] base {b:?}");
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();

    let cfg = PolymarketFileConfig::load(CONFIG_PATH);
    let markets: Vec<_> = cfg
        .polymarket
        .markets
        .iter()
        .filter(|m| m.enabled)
        .cloned()
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {CONFIG_PATH}");
        std::process::exit(1);
    }

    let creds = PolymarketCredentials::load(CREDENTIALS_PATH);

    eprintln!("[boot] endpoint: {}", polymarket::ws::USER_STREAM);
    for m in &markets {
        eprintln!("[boot] market: {} (interval={}s)", m.slug, m.interval_secs);
    }

    let _schedulers: Vec<_> = markets
        .into_iter()
        .map(|m| {
            let creds = creds.clone();
            let base = m.slug.clone();
            let interval = m.interval_secs;
            tokio::spawn(async move {
                run_rolling_scheduler(base.clone(), interval, move |full_slug| {
                    let creds = creds.clone();
                    let base = base.clone();
                    async move { spawn_user_channel_task(base, full_slug, creds).await }
                })
                .await;
            })
        })
        .collect();

    tokio::signal::ctrl_c().await?;
    eprintln!("\n[boot] Ctrl-C received, exiting.");
    Ok(())
}
