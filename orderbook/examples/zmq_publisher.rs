/// Streams live orderbook snapshots over ZMQ.
///
/// PUB  socket: tcp://*:5555  — subscribe to receive data
/// ROUTER socket: tcp://*:5556 — send subscription requests
///
/// Connect with Python:
///   python3 orderbook/examples/zmq_subscriber.py --type snapshot --interval 500 --symbol BTCUSDT
///   python3 orderbook/examples/zmq_subscriber.py --type bba --interval 100 --symbol ETHUSDT
///
/// Dynamically add a new channel:
///   python3 orderbook/examples/zmq_subscriber.py --add-channel --exchange binance --symbol SOLUSDT
use anyhow::Result;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::publisher::ZmqPublisher;
use orderbook::types::ExchangeName;
use orderbook::{OrderbookSystem, OrderbookSystemConfig};
use tracing::info;

const PUB_ENDPOINT: &str = "tcp://*:5555";
const ROUTER_ENDPOINT: &str = "tcp://*:5556";
const DEPTH_LEVELS: usize = 10;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info,zmq_publisher=info")
        .init();

    let mut config = OrderbookSystemConfig::new();
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["btcusdt", "ethusdt"])
                .build(),
        ),
    );
    config.validate()?;

    let system_control = SystemControl::new();
    let mut system = OrderbookSystem::new(config, system_control.clone())?;

    // Get the channel request sender before attaching the publisher
    let channel_tx = system.channel_request_sender();

    // Attach the ZMQ publisher — subscribes to the engine internally
    system.attach_zmq_publisher(ZmqPublisher::new(
        PUB_ENDPOINT,
        ROUTER_ENDPOINT,
        DEPTH_LEVELS,
        channel_tx,
    ));

    info!("ZMQ PUB  on {PUB_ENDPOINT}");
    info!("ZMQ ROUTER on {ROUTER_ENDPOINT}");
    info!(
        "Connect: python3 orderbook/examples/zmq_subscriber.py --type snapshot --interval 500 --symbol BTCUSDT"
    );

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C, shutting down");
            system_control.shutdown();
        }
        result = system.run() => {
            result?;
        }
    }

    Ok(())
}
