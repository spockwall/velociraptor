---
name: velociraptor-rust-api
description: Rust-side API for embedding velociraptor — StreamEngine/StreamSystem construction, HookRegistry, broadcast subscribers, Orderbook query API, per-exchange subscription builders. Use whenever writing Rust code that consumes or extends the orderbook engine.
---

# Velociraptor — Rust API

## Construction pattern

Register hooks **before** handing the engine to `StreamSystem`:

```rust
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use orderbook::connection::{ClientConfig, SystemControl};
use libs::protocol::{ExchangeName, OrderbookSnapshot};

let mut engine = StreamEngine::new(1024, 20);  // (broadcast_capacity, snapshot_depth)

engine.hooks_mut().on::<OrderbookSnapshot, _>(|s: &OrderbookSnapshot| {
    println!("[{}:{}] bid={:?} ask={:?}", s.exchange, s.symbol, s.best_bid, s.best_ask);
});

let mut cfg = StreamSystemConfig::new();
cfg.with_exchange(
    ClientConfig::new(ExchangeName::Binance)
        .set_subscription_message(
            BinanceSubMsgBuilder::new().with_orderbook_channel(&["btcusdt"]).build()
        ),
);

let system = StreamSystem::new(engine, cfg, SystemControl::new())?;
system.run().await?;
```

## StreamEngine surface

```rust
StreamEngine::new(broadcast_capacity: usize, snapshot_depth: usize) -> Self
engine.get_message_sender() -> mpsc::UnboundedSender<StreamMessage>
engine.subscribe_event()    -> broadcast::Receiver<StreamEvent>
engine.bus()                -> StreamEngineBus   // cloneable StreamEventSource
engine.get_orderbooks()     -> Arc<DashMap<String, Arc<Orderbook>>>
engine.hooks_mut()          -> &mut HookRegistry
engine.start(ctrl)          -> StreamEngineHandle
```

Defaults: `event_broadcast_capacity = 1024`, `snapshot_depth = 20`.

## HookRegistry

```rust
engine.hooks_mut().on::<OrderbookSnapshot, _>(|s| { ... });
engine.hooks_mut().on::<OrderbookUpdate,   _>(|u| { ... });
engine.hooks_mut().on::<UserEvent,         _>(|e| { ... });
```

Hooks fire **synchronously inside the engine task**, after broadcast, in registration order. Keep handlers cheap — a slow hook slows the loop.

## StreamEvent variants

| Variant | When | Payload |
|---|---|---|
| `OrderbookRaw(OrderbookUpdate)` | Before update applied | Raw wire data |
| `OrderbookSnapshot(OrderbookSnapshot)` | After update applied | Full materialized book |
| `User(UserEvent)` | On private channel event | Fill / OrderUpdate / Balance / Position |

## Broadcast subscribers (async, out-of-task)

```rust
let mut rx = engine.subscribe_event();   // before StreamSystem::new()

tokio::spawn(async move {
    while let Ok(ev) = rx.recv().await {
        match ev {
            StreamEvent::OrderbookSnapshot(s) => { /* ... */ }
            StreamEvent::User(u) => { /* ... */ }
            _ => {}
        }
    }
});
```

## Direct orderbook poll

```rust
let books = system.orderbooks();
if let Some(ob) = books.get("binance:BTCUSDT") {
    let (bids, asks) = ob.depth(5);
    println!("spread={:.4}", ob.spread().unwrap_or(0.0));
}
```

## `OrderbookSnapshot` fields

```rust
pub exchange:  ExchangeName
pub symbol:    String
pub sequence:  u64
pub ex_timestamp:   i64             // exchange Unix ns (0 when unavailable)
pub recv_timestamp: i64             // local receive Unix ns
pub best_bid:  Option<(f64, f64)>   // (price, qty)
pub best_ask:  Option<(f64, f64)>
pub spread:    Option<f64>
pub mid:       Option<f64>
pub wmid:      f64                  // quantity-weighted mid
pub bids:      Vec<(f64, f64)>      // best first
pub asks:      Vec<(f64, f64)>
```

## `Orderbook` API

```rust
ob.best_bid()       // Option<(price, qty)>
ob.best_ask()
ob.spread()         // Option<f64>
ob.mid_price()
ob.wmid()           // f64
ob.depth(n)         // (Vec<(f64,f64)>, Vec<(f64,f64)>) top-n bids/asks
ob.avg_bid_price(n)
ob.avg_ask_price(n)
ob.vamp(n)          // volume-weighted average mid price
```

## Subscription builders

### Binance (futures depth only)

```rust
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .build()   // -> String
```

### Binance Spot (depth + trades)

```rust
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .with_trade_channel(&["btcusdt", "ethusdt"])
    .build()   // pair with ExchangeName::BinanceSpot
```

### OKX

```rust
OkxSubMsgBuilder::new()
    .with_orderbook_channel_multi(vec!["BTC-USDT", "ETH-USDT-SWAP"], "SPOT")
    .build()
```

### Polymarket

```rust
PolymarketSubMsgBuilder::new()
    .with_asset("71321045679252212594626385532706912750332728571942532289631379312455583992563")
    .build()
```

### Hyperliquid — `build()` returns `Vec<String>` (one per coin)

```rust
HyperliquidSubMsgBuilder::new()
    .with_coins(&["BTC", "ETH", "SOL"])
    .build()

ClientConfig::new(ExchangeName::Hyperliquid)
    .set_subscription_messages(builder.build())   // plural setter
```

### Kalshi

```rust
KalshiSubMsgBuilder::new()
    .with_ticker("KXBTC15M-26APR160700-00")
    .build()
```

## StreamSystemConfig

```rust
let mut cfg = StreamSystemConfig::new();
cfg.with_exchange(client_config);
cfg.set_event_broadcast_capacity(2048);
cfg.set_snapshot_depth(10);
cfg.validate()?;
```
