use anyhow::Result;
use clap::Parser;
use orderbook::api::{OrderbookSystem, OrderbookSystemConfig};
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use std::time::Duration;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "Real-time orderbook connector using OrderbookSystem API"
)]
struct Args {
    /// Enable BitMEX
    #[clap(long, default_value_t = true)]
    bitmex: bool,

    /// Enable OKX
    #[clap(long, default_value_t = true)]
    okx: bool,

    /// Enable BitMart
    #[clap(long, default_value_t = true)]
    bitmart: bool,

    /// Enable Binance
    #[clap(long, default_value_t = true)]
    binance: bool,

    /// Print orderbook stats interval in seconds
    #[clap(long, default_value_t = 5)]
    stats_interval: u64,
}

fn build_system_config(args: &Args) -> Result<OrderbookSystemConfig> {
    let mut config = OrderbookSystemConfig::new();
    config.set_stats_interval(Duration::from_secs(args.stats_interval));

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

    // Validate configuration
    config.validate()?;

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info")
        .init();

    let args = Args::parse();

    info!("Starting orderbook system");
    info!("Stats interval: {} seconds", args.stats_interval);

    // Build system configuration
    let config = build_system_config(&args)?;

    // Create and run the orderbook system
    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down");
            system_control.shutdown();
        }
        result = system.run(true) => {
            if let Err(e) = result {
                error!("System error: {}", e);
                system_control.shutdown();
            }
            info!("System shutdown complete");
        }
    }

    info!("Application shutdown complete");
    Ok(())
}
