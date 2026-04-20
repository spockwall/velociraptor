//! Polymarket `last_trade_price` stream.
//!
//! Connects to the **public** market channel, subscribes to the token IDs
//! resolved from `configs/polymarket.yaml`, and prints every
//! `last_trade_price` event to stderr as it arrives.
//!
//! Runs with the same rolling-window scheduler as `polymarket_orderbook.rs`.
//! Window rotation is handled transparently — the example stays running across
//! 15-min boundaries.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example polymarket_last_trade
//! ```

use anyhow::Result;
use libs::configs::{PolymarketFileConfig, PolymarketMarketConfig};
use libs::protocol::{ExchangeName, LastTradePrice};
use orderbook::PolymarketClient;
use orderbook::connection::{ClientConfig, ClientTrait, SystemControl};
use orderbook::exchanges::polymarket::{
    PolymarketSubMsgBuilder, WindowTask, resolve_assets_with_labels, run_rolling_scheduler,
};
use orderbook::types::orderbook::StreamMessage;
use tokio::sync::mpsc;

const CONFIG_PATH: &str = "configs/polymarket.yaml";

// ── Per-window task ───────────────────────────────────────────────────────────

/// Spawn a public-channel connection for one window slug.
///
/// Resolves token IDs, subscribes to the market channel, and spawns a printer
/// task that logs each `last_trade_price` event. Returns a `WindowTask` the
/// scheduler manages.
async fn spawn_trade_task(
    market: &PolymarketMarketConfig,
    base_slug: String,
    full_slug: String,
) -> Option<WindowTask> {
    let single = vec![PolymarketMarketConfig {
        slug: full_slug.clone(),
        interval_secs: 0,
        ..market.clone()
    }];

    let labeled = tokio::task::spawn_blocking({
        let single = single.clone();
        move || resolve_assets_with_labels(&single)
    })
    .await
    .ok()?;

    if labeled.is_empty() {
        eprintln!("[trade] {full_slug}: no tokens yet (market not open?)");
        return None;
    }

    let asset_ids: Vec<String> = labeled.iter().map(|(id, _, _, _)| id.clone()).collect();
    eprintln!(
        "[trade] {full_slug}: subscribing to {} token(s): {:?}",
        asset_ids.len(),
        asset_ids
    );

    let sub_json = {
        let mut builder = PolymarketSubMsgBuilder::new();
        for id in &asset_ids {
            builder = builder.with_asset(id);
        }
        builder.build()
    };

    let cfg = ClientConfig::new(ExchangeName::Polymarket).set_subscription_message(sub_json);

    let (tx, mut rx) = mpsc::unbounded_channel::<StreamMessage>();
    let control = SystemControl::new();

    let mut conn = PolymarketClient::new(cfg, tx, control.clone());
    let conn_ctrl = control.clone();
    let conn_label = full_slug.clone();
    let conn_task = tokio::spawn(async move {
        if let Err(e) = conn.run().await {
            eprintln!("[trade] {conn_label} conn exited: {e:?}");
        }
        conn_ctrl.shutdown();
    });

    let label = base_slug.clone();
    let full = full_slug.clone();
    let printer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let StreamMessage::LastTradePrice(trade) = msg {
                print_trade(&label, &full, &trade);
            }
        }
    });

    let handle = tokio::spawn(async move {
        let _ = conn_task.await;
        printer.abort();
    });

    Some(WindowTask::new(full_slug, control, handle))
}

// ── Printer ───────────────────────────────────────────────────────────────────

fn print_trade(base: &str, full: &str, t: &LastTradePrice) {
    eprintln!(
        "[{base}@{full}] TRADE {} px={:.4} sz={:.6} fee={} ts={}",
        t.side,
        t.price,
        t.size,
        t.fee_rate_bps,
        t.timestamp.format("%H:%M:%S%.3f"),
    );
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();

    let cfg = PolymarketFileConfig::load(CONFIG_PATH);
    let markets: Vec<PolymarketMarketConfig> = cfg
        .polymarket
        .markets
        .into_iter()
        .filter(|m| m.enabled)
        .collect();

    if markets.is_empty() {
        eprintln!("No enabled markets in {CONFIG_PATH}");
        return Ok(());
    }

    eprintln!("Watching last_trade_price on {} market(s)…", markets.len());

    let ctrl = SystemControl::new();

    let mut scheduler_handles = Vec::new();
    for market in &markets {
        let market_clone: PolymarketMarketConfig = market.clone();
        let base_slug = market.slug.clone();
        let ctrl_clone = ctrl.clone();

        let handle = tokio::spawn(async move {
            run_rolling_scheduler(base_slug.clone(), market_clone.interval_secs, |full_slug| {
                let m = market_clone.clone();
                let base = base_slug.clone();
                async move { spawn_trade_task(&m, base, full_slug).await }
            })
            .await;
            ctrl_clone.shutdown();
        });

        scheduler_handles.push(handle);
    }

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down…");
    ctrl.shutdown();

    for h in scheduler_handles {
        let _ = h.await;
    }

    Ok(())
}
