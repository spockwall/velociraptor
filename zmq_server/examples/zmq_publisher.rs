/// Streams live orderbook snapshots over ZMQ.
///
/// PUB  socket: tcp://*:5555  — subscribe to receive data
/// ROUTER socket: tcp://*:5556 — send subscription requests
///
/// Connect with Python:
///   python3 zmq_server/examples/zmq_subscriber.py --type snapshot --interval 500 --symbol BTCUSDT
///   python3 zmq_server/examples/zmq_subscriber.py --type bba --interval 100 --symbol ETHUSDT
///
/// Dynamically add a new channel:
///   python3 zmq_server/examples/zmq_subscriber.py --add-channel --exchange binance --symbol SOLUSDT
///   python3 zmq_server/examples/zmq_subscriber.py --add-channel --exchange polymarket --symbol <asset_id>
use anyhow::Result;
use libs::protocol::ExchangeName;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::polymarket::PolymarketSubMsgBuilder;
use std::sync::Arc;
use tracing::info;
use zmq_server::{TradingSystem, TradingSystemConfig, ZmqServer};

const PUB_ENDPOINT: &str = "tcp://*:5555";
const ROUTER_ENDPOINT: &str = "tcp://*:5556";
const USER_PUB_ENDPOINT: &str = "tcp://*:5557";
const DEPTH_LEVELS: usize = 10;

// Example Polymarket asset ID — replace with an active market token ID.
const POLYMARKET_ASSET: &str =
    "71321045679252212594626385532706912750332728571942532289631379312455583992563";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info,zmq_server=info,zmq_publisher=info")
        .init();

    let mut config = TradingSystemConfig::new();
    config.set_snapshot_depth(DEPTH_LEVELS);
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["btcusdt", "ethusdt"])
                .build(),
        ),
    );
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::Polymarket).set_subscription_message(
            PolymarketSubMsgBuilder::new()
                .with_asset(POLYMARKET_ASSET)
                .build(),
        ),
    );
    config.validate()?;

    let system_control = SystemControl::new();
    let mut system = TradingSystem::new(config, system_control.clone())?;

    let channel_tx = system.channel_request_sender();
    let zmq_handle = ZmqServer::new(PUB_ENDPOINT, ROUTER_ENDPOINT, USER_PUB_ENDPOINT, channel_tx)
        .start(Arc::new(system.engine_bus()));
    system.attach_handle(zmq_handle);

    info!("ZMQ PUB  on {PUB_ENDPOINT}");
    info!("ZMQ ROUTER on {ROUTER_ENDPOINT}");
    info!(
        "Connect: python3 zmq_server/examples/zmq_subscriber.py --type snapshot --interval 500 --symbol BTCUSDT"
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
