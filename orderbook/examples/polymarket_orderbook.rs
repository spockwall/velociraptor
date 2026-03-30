/// Subscribe to one or more Polymarket assets and print orderbook snapshots.
///
/// Usage:
///   cargo run --example polymarket_orderbook -- --asset <yes_token_id> --asset <no_token_id>
///
/// Get token IDs:
///   python3 scripts/fetch_polymarket_tokens.py --search "bitcoin"
use anyhow::Result;
use clap::Parser;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::types::ExchangeName;
use orderbook::{OrderbookEvent, OrderbookSystem, OrderbookSystemConfig, PolymarketSubMsgBuilder};
use std::time::Duration;

const DEFAULT_YES: &str =
    "75467129615908319583031474642658885479135630431889036121812713428992454630178";
const DEFAULT_NO: &str =
    "3842963720267267286970642336860752782302644680156535061700039388405652129691";

#[derive(Parser, Debug)]
#[clap(about = "Polymarket orderbook viewer — subscribes to both Yes and No tokens")]
struct Args {
    /// Polymarket asset (token) IDs to subscribe to (pass --asset twice for Yes + No)
    #[clap(long = "asset", num_args = 1, action = clap::ArgAction::Append)]
    assets: Vec<String>,

    /// How long to run in seconds (0 = run until Ctrl+C)
    #[clap(long, default_value_t = 0)]
    duration: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("orderbook=warn")
        .try_init();

    let mut args = Args::parse();

    // Default to the BitBoy convicted? Yes+No pair if no assets given
    if args.assets.is_empty() {
        args.assets.push(DEFAULT_YES.to_string());
        args.assets.push(DEFAULT_NO.to_string());
    }

    println!("Subscribing to {} asset(s):", args.assets.len());
    for a in &args.assets {
        println!("  {a}");
    }

    let mut builder = PolymarketSubMsgBuilder::new();
    for asset in &args.assets {
        builder = builder.with_asset(asset);
    }

    let mut config = OrderbookSystemConfig::new();
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::Polymarket).set_subscription_message(builder.build()),
    );
    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    let _handle = system.on_update(move |event| async move {
        match event {
            OrderbookEvent::Snapshot(snap) => {
                let (bids, asks) = snap.book.depth(5);
                println!(
                    "[{}] asset={}...{} seq={} best_bid={:?} best_ask={:?} spread={:?}",
                    snap.exchange,
                    &snap.symbol[..8],
                    &snap.symbol[snap.symbol.len() - 4..],
                    snap.sequence,
                    snap.book.best_bid(),
                    snap.book.best_ask(),
                    snap.book.spread(),
                );
                if !bids.is_empty() || !asks.is_empty() {
                    println!("  asks (worst→best): {:?}", asks);
                    println!("  bids (best→worst): {:?}", bids);
                }
            }
            OrderbookEvent::RawUpdate(_) => {}
        }
    });

    if args.duration > 0 {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(args.duration)) => {
                println!("Done after {}s", args.duration);
                system_control.shutdown();
            }
            _ = tokio::signal::ctrl_c() => {
                system_control.shutdown();
            }
            result = system.run() => {
                result?;
            }
        }
    } else {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                system_control.shutdown();
            }
            result = system.run() => {
                result?;
            }
        }
    }

    Ok(())
}
