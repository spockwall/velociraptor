use orderbook::api::{OrderbookSystem, OrderbookSystemConfig};
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use orderbook::{OrderbookEvent};
use std::time::Duration;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info")
        .init();

    info!("Starting OrderbookSystem API example");

    let mut config = OrderbookSystemConfig::new();

    config
        .with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
                BinanceSubMsgBuilder::new()
                    .with_orderbook_channel(&["btcusdt", "ethusdt"])
                    .build(),
            ),
        )
        .with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
                OkxSubMsgBuilder::new()
                    .with_orderbook_channel("BTC-USDT-SWAP", "SWAP")
                    .build(),
            ),
        )
        .set_stats_interval(Duration::from_secs(5));

    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    // Register a snapshot callback — called after every update with the full book
    system.on_update(|event| async move {
        if let OrderbookEvent::Snapshot(snap) = event {
            // Full Orderbook methods are available on snap.book
            if let (Some((bid, bid_qty)), Some((ask, ask_qty))) =
                (snap.book.best_bid(), snap.book.best_ask())
            {
                info!(
                    "[{}:{}] bid={:.4} ({:.4}) ask={:.4} ({:.4}) spread={:.4} wmid={:.4}",
                    snap.exchange,
                    snap.symbol,
                    bid,
                    bid_qty,
                    ask,
                    ask_qty,
                    ask - bid,
                    snap.book.wmid(),
                );
            }
        }
    });

    // Direct orderbook access is still available
    let orderbooks = system.orderbooks();
    info!("Tracking {} orderbooks", orderbooks.len());

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down");
            system_control.shutdown();
        }
        result = system.run() => {
            if let Err(e) = result {
                error!("System error: {}", e);
            }
        }
    }

    info!("Application shutdown complete");
    Ok(())
}
