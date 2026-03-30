use anyhow::Result;
use clap::Parser;
use libs::terminal::OrderbookUi;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use orderbook::{OrderbookEvent, OrderbookSystem, OrderbookSystemConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "Real-time orderbook visualizer — side-by-side panels"
)]
struct Args {
    /// Enable OKX symbols
    #[clap(long, default_value_t = true)]
    okx: bool,

    /// Enable Binance symbols
    #[clap(long, default_value_t = true)]
    binance: bool,

    /// Enable Hyperliquid symbols
    #[clap(long, default_value_t = true)]
    hyperliquid: bool,

    /// Depth levels to display per side
    #[clap(long, default_value_t = 8)]
    depth: usize,

    /// Render interval in milliseconds
    #[clap(long, default_value_t = 300)]
    interval: u64,
}

// ── Shared state written by on_update, read by the render timer ───────────────

struct SnapState {
    exchange: String,
    symbol: String,
    sequence: u64,
    best_bid: Option<(f64, f64)>,
    best_ask: Option<(f64, f64)>,
    spread: Option<f64>,
    mid: Option<f64>,
    wmid: f64,
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
}

type SnapStore = Arc<Mutex<HashMap<String, SnapState>>>;

fn build_system_config(args: &Args) -> Result<OrderbookSystemConfig> {
    let mut config = OrderbookSystemConfig::new();

    if args.okx {
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
                OkxSubMsgBuilder::new()
                    .with_orderbook_channel("BTC-USDT-SWAP", "SWAP")
                    .with_orderbook_channel("ETH-USDT-SWAP", "SWAP")
                    .build(),
            ),
        );
    }

    if args.binance {
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
                BinanceSubMsgBuilder::new()
                    .with_orderbook_channel(&["btcusdt", "ethusdt"])
                    .build(),
            ),
        );
    }

    if args.hyperliquid {
        config.with_exchange(
            ConnectionConfig::new(ExchangeName::Hyperliquid).set_subscription_message(
                HyperliquidSubMsgBuilder::new()
                    .with_coins(&["BTC", "ETH"])
                    .build(),
            ),
        );
    }

    config.validate()?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("off").try_init();

    let args = Args::parse();
    let depth = args.depth;
    let interval = Duration::from_millis(args.interval);

    let config = build_system_config(&args)?;
    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    // Shared store: on_update writes here at full exchange rate.
    let store: SnapStore = Arc::new(Mutex::new(HashMap::new()));

    // on_update — just update the store, no rendering.
    let store_writer = store.clone();
    let _event_handle = system.on_update(move |event| {
        let store = store_writer.clone();
        async move {
            if let OrderbookEvent::Snapshot(snap) = event {
                let (bids, asks) = snap.book.depth(depth);
                let key = format!("{}:{}", snap.exchange, snap.symbol);
                if let Ok(mut map) = store.lock() {
                    map.insert(
                        key,
                        SnapState {
                            exchange: snap.exchange.to_string(),
                            symbol: snap.symbol.clone(),
                            sequence: snap.sequence,
                            best_bid: snap.book.best_bid(),
                            best_ask: snap.book.best_ask(),
                            spread: snap.book.spread(),
                            mid: snap.book.mid_price(),
                            wmid: snap.book.wmid(),
                            bids,
                            asks,
                        },
                    );
                }
            }
        }
    });

    // Render timer — reads the store and redraws at the configured interval.
    let store_reader = store.clone();
    let _render_handle = tokio::spawn(async move {
        let mut ui = match OrderbookUi::new(depth) {
            Ok(u) => u,
            Err(_) => return,
        };
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            // Collect a snapshot of all current data while holding the lock briefly.
            let entries: Vec<_> = {
                let Ok(map) = store_reader.lock() else {
                    continue;
                };
                map.values()
                    .map(|s| {
                        (
                            s.exchange.clone(),
                            s.symbol.clone(),
                            s.sequence,
                            s.best_bid,
                            s.best_ask,
                            s.spread,
                            s.mid,
                            s.wmid,
                            s.bids.clone(),
                            s.asks.clone(),
                        )
                    })
                    .collect()
            };
            for (exchange, symbol, seq, best_bid, best_ask, spread, mid, wmid, bids, asks) in
                &entries
            {
                ui.render(
                    exchange, symbol, *seq, *best_bid, *best_ask, *spread, *mid, *wmid, bids, asks,
                );
            }
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            system_control.shutdown();
        }
        result = system.run() => {
            result?;
        }
    }

    Ok(())
}
