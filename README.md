# velociraptor

A high-performance Rust library for real-time cryptocurrency orderbook streaming from major exchanges.

## Workspace

```
velociraptor/
├── libs/        # Shared constants and exchange endpoint definitions
└── orderbook/   # Core orderbook library
```

## Supported Exchanges

| Exchange | Status  | Stream Type              |
|----------|---------|--------------------------|
| Binance  | ✅ Live | Partial Book Depth 20 @ 100ms (`fstream.binance.com`) |
| OKX      | ✅ Live | `books` channel (snapshot + incremental updates)      |

## Quick Start

```toml
[dependencies]
orderbook = { path = "orderbook" }
```

```rust
use orderbook::api::{OrderbookSystem, OrderbookSystemConfig};
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::types::ExchangeName;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info")
        .init();

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
                    .with_orderbook_channel("ETH-USDT-SWAP", "SWAP")
                    .build(),
            ),
        )
        .set_stats_interval(Duration::from_secs(5));

    config.validate()?;

    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    // Access the live orderbook DashMap
    if let Some(orderbooks) = system.get_orderbooks() {
        if let Some(ob) = orderbooks.get("BTCUSDT") {
            if let (Some((bid, _)), Some((ask, _))) = (ob.best_bid(), ob.best_ask()) {
                println!("BTC bid={bid:.2} ask={ask:.2} spread={:.2}", ask - bid);
            }
        }
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => { system_control.shutdown(); }
        result = system.run(true) => { result?; }
    }

    Ok(())
}
```

## Orderbook API

The `Orderbook` struct exposes price-level state via `BTreeMap<OrderedFloat<f64>, PriceLevel>`:

```rust
ob.best_bid()           // Option<(price, qty)>
ob.best_ask()           // Option<(price, qty)>
ob.spread()             // Option<f64>
ob.mid_price()          // Option<f64>
ob.depth(n)             // (Vec<(price, qty)>, Vec<(price, qty)>)
ob.avg_bid_price(n)     // f64 — average of top-n bid prices
ob.avg_ask_price(n)     // f64 — average of top-n ask prices
ob.vamp(n)              // f64 — volume-weighted average mid price
ob.wmid()               // f64 — quantity-weighted mid price
```

## Subscription Builders

### Binance

Subscribes to `@depth20@100ms` (Partial Book Depth, 20 levels) on `fstream.binance.com/ws`. Every message is a full snapshot — no sequencing needed.

```rust
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .build()
```

### OKX

Subscribes to the `books` channel on `wss://ws.okx.com:8443/ws/v5/public`. First message is a snapshot (`action: "snapshot"`), subsequent messages are incremental updates (`action: "update"`). Zero-quantity entries remove a price level.

```rust
OkxSubMsgBuilder::new()
    .with_orderbook_channel("BTC-USDT-SWAP", "SWAP")
    .with_orderbook_channel_multi(vec!["ETH-USDT", "SOL-USDT"], "SPOT")
    .build()
```

## ConnectionConfig Options

```rust
ConnectionConfig::new(ExchangeName::Binance)
    .set_subscription_message(msg)
    .set_ping_interval(15)           // seconds, default 15
    .set_reconnect_delay(5)          // seconds, default 5
    .set_max_reconnect_attempts(10)  // default 10
    .set_ws_url("wss://custom-url")  // override default endpoint
```

## Commands

```bash
cargo build
cargo build --release

cargo run --example stream_orderbook_instance
cargo run --example api_usage
cargo run --example exchange_websocket
cargo run --example exchange_connector_tests

cargo test
cargo fmt
cargo clippy
```
