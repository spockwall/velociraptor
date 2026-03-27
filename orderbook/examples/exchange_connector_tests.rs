use anyhow::Ok;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::ConnectionFactory;
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use orderbook::types::endpoints::{binance, okx};

async fn test_bitmex_connection() -> Result<(), anyhow::Error> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let system_control = SystemControl::new();
    let factory = ConnectionFactory {
        config: ConnectionConfig {
            exchange: ExchangeName::Binance,
            subscription_message: BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["ETHUSD"])
                .build(),
            ws_url: binance::ws::PUBLIC_STREAM.to_string(),
            ping_interval: 10,
            reconnect_delay: 10,
            max_reconnect_attempts: 10,
            api_key: None,
            api_secret: None,
            passphrase: None,
        },
        message_tx: tx,
    };

    let mut connection = factory.create_connection(system_control);
    let timeout = std::time::Duration::from_secs(10);
    let _res = tokio::time::timeout(timeout, connection.run()).await?;

    Ok(())
}

async fn test_okx_connection() -> Result<(), anyhow::Error> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let system_control = SystemControl::new();
    let factory = ConnectionFactory {
        config: ConnectionConfig {
            exchange: ExchangeName::Okx,
            subscription_message: OkxSubMsgBuilder::new()
                .with_orderbook_channel("BTC-USDT", "SPOT")
                .build(),
            ws_url: okx::ws::PUBLIC_STREAM.to_string(),
            ping_interval: 10,
            reconnect_delay: 10,
            max_reconnect_attempts: 10,
            api_key: None,
            api_secret: None,
            passphrase: None,
        },
        message_tx: tx,
    };

    let mut connection = factory.create_connection(system_control);
    let timeout = std::time::Duration::from_secs(10);
    let _res = tokio::time::timeout(timeout, connection.run()).await?;

    Ok(())
}

#[tokio::main]
async fn main() {
    let _ = test_bitmex_connection().await;
    let _ = test_okx_connection().await;
}
