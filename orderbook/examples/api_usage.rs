use orderbook::api::{OrderbookSystem, OrderbookSystemConfig};
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use std::time::Duration;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt().with_env_filter("debug").init();

    info!("Starting OrderbookSystem API example");

    // Example 1: Basic usage with BitMEX
    basic_bitmex_example().await?;

    // Example 2: Multi-exchange setup
    multi_exchange_example().await?;

    // Example 3: Custom configuration
    custom_config_example().await?;

    // Example 4: Orderbook manager disabled (message consumer only)
    message_consumer_only_example().await?;

    // Example 5: Error handling and validation
    error_handling_example().await?;

    // Example 6: Real-time orderbook monitoring
    realtime_monitoring_example().await?;

    Ok(())
}

/// Example 1: Basic usage with BitMEX
async fn basic_bitmex_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 1: Basic BitMEX Setup ===");

    let mut config = OrderbookSystemConfig::new();

    // Add BitMEX with BTCUSD and ETHUSD symbols
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["XBTUSD", "ETHUSD"])
                .build(),
        ),
    );

    // Validate configuration
    config.validate()?;

    // Create and run the system
    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control)?;

    // Get access to orderbooks (if enabled)
    if let Some(_orderbooks) = system.get_orderbooks() {
        info!("Orderbook manager is enabled");

        // You can access orderbooks here
        // For example, to get BTCUSD orderbook:
        // if let Some(btc_orderbook) = orderbooks.get("XBTUSD") {
        //     println!("BTC Best Bid: {:?}", btc_orderbook.best_bid());
        //     println!("BTC Best Ask: {:?}", btc_orderbook.best_ask());
        // }
    }

    // Run the system (this will block until shutdown)
    // In a real application, you might want to run this in a separate thread
    // Uncomment the line below to actually run the system and see connection details
    system.run(true).await?;

    info!("Basic BitMEX example completed");
    Ok(())
}

/// Example 2: Multi-exchange setup
async fn multi_exchange_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 2: Multi-Exchange Setup ===");

    let mut config = OrderbookSystemConfig::new();

    // Add multiple exchanges
    config
        .with_exchange(
            ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
                BinanceSubMsgBuilder::new()
                    .with_orderbook_channel(&["XBTUSD"])
                    .build(),
            ),
        )
        .with_exchange(
            ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
                OkxSubMsgBuilder::new()
                    .with_orderbook_channel("BTC-USDT", "SWAP")
                    .build(),
            ),
        );

    // Customize stats interval
    config.set_stats_interval(Duration::from_secs(5));

    // Validate configuration
    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control)?;

    if let Some(_orderbooks) = system.get_orderbooks() {
        info!("Multi-exchange orderbook manager is enabled");
        info!("Tracking orderbooks");
    }

    system.run(true).await?;

    info!("Multi-exchange example completed");
    Ok(())
}

/// Example 3: Custom configuration with manual exchange setup
async fn custom_config_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 3: Custom Configuration ===");

    let mut config = OrderbookSystemConfig::new();

    // Manually configure BitMEX with custom settings
    let bitmex_config = ConnectionConfig::new(ExchangeName::Binance)
        .set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["XBTUSD", "ETHUSD"])
                .build(),
        )
        .set_ping_interval(15) // 15 seconds
        .set_reconnect_delay(3) // 3 seconds
        .set_max_reconnect_attempts(5);

    config
        .with_exchange(bitmex_config)
        .set_channel_buffer_size(50_000)
        .set_stats_interval(Duration::from_secs(15));

    // Validate configuration
    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control)?;

    if let Some(_orderbooks) = system.get_orderbooks() {
        info!("Custom configuration orderbook manager is enabled");
    }

    // system.run()?;

    info!("Custom configuration example completed");
    Ok(())
}

/// Example 4: Orderbook manager disabled (just message consumer)
async fn message_consumer_only_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 4: Message Consumer Only ===");

    let mut config = OrderbookSystemConfig::new();
    let bitmex_config = ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
        BinanceSubMsgBuilder::new()
            .with_orderbook_channel(&["XBTUSD"])
            .build(),
    );
    let okx_config = ConnectionConfig::new(ExchangeName::Okx).set_subscription_message(
        OkxSubMsgBuilder::new()
            .with_orderbook_channel("BTC-USDT", "SPOT")
            .build(),
    );

    // Add exchanges
    config
        .with_exchange(bitmex_config)
        .with_exchange(okx_config);

    // Validate configuration
    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control)?;

    // Orderbook manager is disabled, so get_orderbooks() returns None
    if system.get_orderbooks().is_none() {
        info!("Orderbook manager is disabled - only consuming messages");
    }

    // system.run()?;

    info!("Message consumer only example completed");
    Ok(())
}

/// Example 5: Error handling and validation
async fn error_handling_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 5: Error Handling ===");

    // Try to create config with no exchanges
    let mut config = OrderbookSystemConfig::new();
    let bitmex_config = ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
        BinanceSubMsgBuilder::new()
            .with_orderbook_channel(&["XBTUSD"])
            .build(),
    );

    match config.validate() {
        Ok(_) => {
            error!("This should not happen - no exchanges configured");
        }
        Err(e) => {
            info!("Expected error: {}", e);
        }
    }

    // Add exchanges and try again
    config.with_exchange(bitmex_config);

    match config.validate() {
        Ok(_) => {
            info!("Configuration is valid");
        }
        Err(e) => {
            error!("Unexpected error: {}", e);
        }
    }

    info!("Error handling example completed");
    Ok(())
}

/// Example 6: Real-time orderbook monitoring
async fn realtime_monitoring_example() -> Result<(), Box<dyn std::error::Error>> {
    info!("=== Example 6: Real-time Monitoring ===");

    let mut config = OrderbookSystemConfig::new();
    let bitmex_config = ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
        BinanceSubMsgBuilder::new()
            .with_orderbook_channel(&["XBTUSD"])
            .build(),
    );
    config
        .with_exchange(bitmex_config)
        .set_stats_interval(Duration::from_secs(2)); // Fast stats updates

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control)?;

    if let Some(_orderbooks) = system.get_orderbooks() {
        info!("Real-time monitoring enabled");

        // In a real application, you would:
        // 1. Spawn a thread to run the system
        // 2. In another thread, continuously monitor orderbooks
        // 3. Process orderbook updates for trading strategies

        // Example monitoring loop (commented out):
        /*
        loop {
            if let Some(btc_orderbook) = orderbooks.get("XBTUSD") {
                if btc_orderbook.is_initialized {
                    if let (Some((bid_price, bid_qty)), Some((ask_price, ask_qty))) =
                        (btc_orderbook.best_bid(), btc_orderbook.best_ask()) {

                        let spread = ask_price - bid_price;
                        let mid_price = (bid_price + ask_price) / 2.0;

                        println!("BTC: Bid={:.2} Ask={:.2} Spread={:.2} Mid={:.2}",
                                bid_price, ask_price, spread, mid_price);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        */
    }

    // system.run()?;

    info!("Real-time monitoring example completed");
    Ok(())
}
