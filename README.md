# velociraptor

A high-performance Rust workspace for real-time market data streaming and order execution on prediction markets and crypto exchanges.

| Exchange    | Status | Stream Type |
|-------------|--------|-------------|
| Binance     | Live   | Partial Book Depth 20 @ 100ms |
| OKX         | Live   | `books` channel — snapshot + incremental updates |
| Polymarket  | Live   | `book` snapshot + `price_change` incremental diffs |
| Hyperliquid | Live   | `l2Book` full snapshot on every update |
| Kalshi      | Live   | `orderbook_snapshot` + signed `orderbook_delta` — requires API key |

---

## Workspace

| Crate | Purpose |
|---|---|
| `libs` | Shared protocol types, configs, credentials, Redis client, terminal UI |
| `orderbook` | Multi-exchange market data engine — `StreamEngine`, `StreamSystem`, connectors |
| `zmq_server` | ZMQ transport layer — PUB (market data), ROUTER (subscriptions), PUB (user events) |
| `recorder` | Append-only MessagePack snapshot writer with daily rotation and zstd compression |
| `executor` | ZMQ REP gateway for REST order placement (Phase 3, stub) |
| `backend` | Axum HTTP API for React frontend (Phase 6, stub) |

---

## Quick Start

### 1. Configure

Edit `configs/server.yaml`:

```yaml
binance:
  enabled: true
  symbols: ["btcusdt", "ethusdt"]

hyperliquid:
  enabled: true
  coins: ["BTC", "ETH"]

kalshi:
  enabled: true
  tickers: ["KXBTC15M-26APR130415-15"]

polymarket:
  markets:
    - enabled: true
      slug: "btc-updown-5m"
      interval_secs: 300

storage:
  enabled: true
  base_path: "./data"
  depth: 20
  flush_interval: 1000
  rotation: "daily"
  zstd_level: 3
```

For Kalshi, set your RSA key pair in `credentials/kalshi.yaml`:

```yaml
kalshi:
  key_id: "<your-key-id-uuid>"
  private_key: |
    -----BEGIN PRIVATE KEY-----
    <your-rsa-private-key-pem>
    -----END PRIVATE KEY-----
```

### 2. Start the server

```bash
cargo run --bin orderbook_server --release -- --config configs/server.yaml
```

### 3. Subscribe from Python

```bash
python3 zmq_server/examples/zmq_subscriber.py \
    --exchange binance --symbol BTCUSDT --type snapshot --interval 500

python3 zmq_server/examples/zmq_subscriber.py \
    --exchange polymarket \
    --symbol 75467129615908319583031474642658885479135630431889036121812713428992454630178 \
    --type bba --interval 100
```

### 4. Read stored snapshots

```bash
python3 scripts/read_mpack.py data/binance/BTCUSDT/
python3 scripts/read_mpack.py data/polymarket/btc-updown-5m/2026-04-05/
```

---

## Running as systemd Services (Linux)

```bash
cargo build --bin orderbook_server --release
cargo build --bin polymarket_recorder --release

sudo cp systemd/orderbook-server.service    /etc/systemd/system/
sudo cp systemd/polymarket-recorder.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now orderbook-server
sudo systemctl enable --now polymarket-recorder

sudo journalctl -u orderbook-server -f
```

See [`docs/systemd.md`](docs/systemd.md) for restart policy details and update procedure.

---

## Polymarket Tools

### Live terminal visualiser

```bash
cargo run --example polymarket_orderbook --release
```

### Disk recorder

Writes every orderbook snapshot per window per side:

```
data/polymarket/btc-updown-5m/2026-04-05/09:55-10:00-up.mpack
                                         09:55-10:00-down.mpack
```

```bash
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml
```

See [`docs/polymarket.md`](docs/polymarket.md) for file format and Python reader.

---

## Kalshi Tools

Kalshi is a CFTC-regulated prediction market. This project streams rolling 15-minute BTC and ETH price direction markets. Authentication is required even for market data.

```bash
cp credentials/example.yaml credentials/kalshi.yaml
# fill in api_key / private_key

cargo run --example kalshi_orderbook --release -- \
    --config configs/kalshi.yaml \
    --credentials credentials/kalshi.yaml
```

See [`docs/kalshi.md`](docs/kalshi.md) for ticker format and scheduler details.

---

## Architecture

Three layers: **exchange connectors** push raw wire data into the **stream engine**, which maintains live book state and fans out typed events to **consumers** (ZMQ transport, hooks, broadcast subscribers).

```
┌──────────────────────────────────────────────────────────────────────┐
│                           StreamSystem                               │
│                                                                      │
│  ┌──────────────┐                                                    │
│  │   Binance    │ ──┐                                               │
│  │   Client     │   │  mpsc::UnboundedSender<StreamMessage>         │
│  └──────────────┘   │                                               │
│                     ▼                                               │
│  ┌──────────────┐  ┌─────────────────────────────────────────────┐  │
│  │     OKX      │ ─► StreamEngine                                │  │
│  │   Client     │  │                                             │  │
│  └──────────────┘  │  DashMap<"exchange:symbol", Arc<Orderbook>> │  │
│                    │                                             │  │
│  ┌──────────────┐  │  HookRegistry  (sync, in-task)             │  │
│  │ Hyperliquid  │ ─►    .on::<StreamSnapshot, _>(|s| ...)        │  │
│  │   Client     │  │    .on::<OrderbookUpdate, _>(|u| ...)       │  │
│  └──────────────┘  │    .on::<UserEvent, _>(|e| ...)             │  │
│                    │                                             │  │
│  ┌──────────────┐  │  broadcast::Sender<StreamEvent>             │  │
│  │   Kalshi     │ ─►    OrderbookRaw(OrderbookUpdate)            │  │
│  │   Client     │  │    OrderbookSnapshot(StreamSnapshot)        │  │
│  └──────────────┘  │    User(UserEvent)                          │  │
│                    └────────────────┬────────────────────────────┘  │
└─────────────────────────────────────│────────────────────────────────┘
                                      │ StreamEngineBus (broadcast subscribe)
                         ┌────────────┴──────────────┐
                         ▼                            ▼
               ┌──────────────────┐        ┌──────────────────┐
               │   ZmqServer      │        │  StorageWriter   │
               │  PUB  market     │        │  (recorder crate)│
               │  ROUTER control  │        └──────────────────┘
               │  PUB  user events│
               └──────────────────┘
```

### Data Flow

```
Exchange WebSocket
  → ClientBase.handle_message()        parse raw text
  → MsgParserTrait.parse_message()     returns Vec<StreamMessage>
  → mpsc channel
  → StreamEngine loop
      ├─► HookRegistry.fire(&update)        (sync, before apply)
      ├─► broadcast OrderbookRaw(update)
      ├─► Orderbook.apply_update(update)    (Arc::make_mut copy-on-write)
      ├─► HookRegistry.fire(&snapshot)      (sync, after apply)
      └─► broadcast OrderbookSnapshot(snap)
```

---

## Rust Usage

### Construction pattern

Register hooks on the engine **before** handing it to `StreamSystem`:

```rust
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use orderbook::connection::{ClientConfig, SystemControl};
use libs::protocol::{ExchangeName, StreamSnapshot};

let mut engine = StreamEngine::new(1024, 20);  // (broadcast_capacity, snapshot_depth)

// Register typed hooks — fire synchronously in the engine task after broadcast.
engine.hooks_mut().on::<StreamSnapshot, _>(move |snap| {
    println!("[{}:{}] bid={:?} ask={:?}", snap.exchange, snap.symbol, snap.best_bid, snap.best_ask);
});

let mut cfg = StreamSystemConfig::new();
cfg.with_exchange(
    ClientConfig::new(ExchangeName::Binance)
        .set_subscription_message(BinanceSubMsgBuilder::new().with_orderbook_channel(&["btcusdt"]).build()),
);

let system_control = SystemControl::new();
let system = StreamSystem::new(engine, cfg, system_control.clone())?;
system.run().await?;
```

### Broadcast subscribers (async, out-of-task)

```rust
let mut rx = engine.subscribe_event();   // before engine.start() / StreamSystem::new()

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

### Direct orderbook poll

```rust
let books = system.orderbooks();
if let Some(ob) = books.get("binance:BTCUSDT") {
    let (bids, asks) = ob.depth(5);
    println!("spread={:.4}", ob.spread().unwrap_or(0.0));
}
```

---

## StreamEngine API

```rust
StreamEngine::new(broadcast_capacity: usize, snapshot_depth: usize) -> Self
engine.get_message_sender()  -> mpsc::UnboundedSender<StreamMessage>
engine.subscribe_event()     -> broadcast::Receiver<StreamEvent>
engine.bus()                 -> StreamEngineBus   // cloneable StreamEventSource
engine.get_orderbooks()      -> Arc<DashMap<String, Arc<Orderbook>>>
engine.hooks_mut()           -> &mut HookRegistry
engine.start(ctrl)           -> StreamEngineHandle
```

### HookRegistry

```rust
// Register — call before engine.start() / StreamSystem::new()
engine.hooks_mut().on::<StreamSnapshot, _>(|s: &StreamSnapshot| { ... });
engine.hooks_mut().on::<OrderbookUpdate, _>(|u: &OrderbookUpdate| { ... });
engine.hooks_mut().on::<UserEvent, _>(|e: &UserEvent| { ... });

// Hooks fire synchronously inside the engine task, after broadcast, in registration order.
// Keep handlers cheap and non-blocking.
```

### StreamEvent variants

| Variant | When | Payload |
|---|---|---|
| `OrderbookRaw(OrderbookUpdate)` | Before update is applied | Raw wire data |
| `OrderbookSnapshot(StreamSnapshot)` | After update is applied | Full materialized book |
| `User(UserEvent)` | On private channel event | Fill, OrderUpdate, Balance, Position |

### StreamSnapshot fields

```rust
pub exchange: ExchangeName
pub symbol:   String
pub sequence: u64
pub timestamp: DateTime<Utc>
pub best_bid:  Option<(f64, f64)>   // (price, qty)
pub best_ask:  Option<(f64, f64)>
pub spread:    Option<f64>
pub mid:       Option<f64>
pub wmid:      f64                  // quantity-weighted mid
pub bids:      Vec<(f64, f64)>      // best first
pub asks:      Vec<(f64, f64)>      // best first
```

---

## StreamSystemConfig API

```rust
let mut cfg = StreamSystemConfig::new();
cfg.with_exchange(client_config);
cfg.set_event_broadcast_capacity(2048);
cfg.set_snapshot_depth(10);
cfg.validate()?;
```

Default values: `event_broadcast_capacity = 1024`, `snapshot_depth = 20`.

---

## Orderbook API

```rust
ob.best_bid()        // Option<(price, qty)>
ob.best_ask()        // Option<(price, qty)>
ob.spread()          // Option<f64>
ob.mid_price()       // Option<f64>
ob.wmid()            // f64 — quantity-weighted mid price
ob.depth(n)          // (Vec<(f64, f64)>, Vec<(f64, f64)>) — top-n bids and asks
ob.avg_bid_price(n)  // f64
ob.avg_ask_price(n)  // f64
ob.vamp(n)           // f64 — volume-weighted average mid price
```

---

## Subscription Builders

### Binance

```rust
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .build()   // -> String
```

### OKX

```rust
OkxSubMsgBuilder::new()
    .with_orderbook_channel("BTC-USDT-SWAP", "SWAP")
    .with_orderbook_channel_multi(vec!["ETH-USDT", "SOL-USDT"], "SPOT")
    .build()   // -> String
```

### Polymarket

```rust
PolymarketSubMsgBuilder::new()
    .with_asset("71321045679252212594626385532706912750332728571942532289631379312455583992563")
    .build()   // -> String

// User (private) channel
PolymarketUserSubMsgBuilder::new()
    .with_auth(&api_key, &secret, &passphrase)
    .with_condition(&condition_id)
    .build()   // -> String
```

### Hyperliquid

```rust
HyperliquidSubMsgBuilder::new()
    .with_coins(&["BTC", "ETH", "SOL"])
    .build()   // -> Vec<String>  (one frame per coin)

// Use set_subscription_messages (plural):
ClientConfig::new(ExchangeName::Hyperliquid)
    .set_subscription_messages(builder.build())
```

### Kalshi

```rust
KalshiSubMsgBuilder::new()
    .with_ticker("KXBTC15M-26APR130415-15")
    .build()   // -> String
```

---

## ZMQ Interface

The server exposes three ZMQ sockets.

### Market PUB socket — `pub_endpoint` (default `tcp://*:5555`)

Publishes throttled orderbook data as two-part frames `[topic, msgpack_payload]`.

Topic format: `"{exchange}:{symbol}"` — e.g. `"binance:BTCUSDT"`, `"polymarket:<token_id>"`

### ROUTER socket — `router_endpoint` (default `tcp://*:5556`)

Clients connect a DEALER socket to send subscription requests and receive acks.

```json
{ "action": "subscribe", "exchange": "binance", "symbol": "BTCUSDT", "type": "snapshot", "interval": 500 }
{ "action": "unsubscribe", "exchange": "binance", "symbol": "BTCUSDT" }
```

| Field | Values | Notes |
|---|---|---|
| `action` | `subscribe` `unsubscribe` | |
| `exchange` | `binance` `okx` `polymarket` `hyperliquid` `kalshi` | |
| `symbol` | e.g. `BTCUSDT`, `BTC-USDT`, `<token_id>` | Exchange-native format |
| `type` | `snapshot` `bba` | Required for subscribe |
| `interval` | milliseconds | Required for subscribe |

Ack format: `{"status": "ok"|"error", "exchange"?: ..., "symbol"?: ..., "type"?: ..., "interval"?: ..., "message"?: ...}`

### User-event PUB socket — `user_pub_endpoint` (default `WS_STATUS_SOCKET`)

Publishes private account events as `[topic, msgpack_payload]`.

Topic format: `"user.{exchange}.{kind}"` — e.g. `"user.polymarket.fill"`, `"user.kalshi.order_update"`

### Data types

| `type` | Published fields |
|---|---|
| `snapshot` | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread`, `mid`, `wmid`, `bids`, `asks` |
| `bba` | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread` |

---

## Python Client

### Minimal snippet — Binance snapshot

```python
import zmq, msgpack

ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    topic, payload = sub.recv_multipart()
    snap = msgpack.unpackb(payload, raw=False)
    print(f"bid={snap['best_bid']}  ask={snap['best_ask']}  wmid={snap['wmid']:.4f}")
```

### Minimal snippet — subscribe via DEALER + read PUB

```python
import zmq, msgpack

ctx = zmq.Context()

dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({"action": "subscribe", "exchange": "binance", "symbol": "BTCUSDT", "type": "snapshot", "interval": 500})
print("ack:", dealer.recv_json())

sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    topic, payload = sub.recv_multipart()
    snap = msgpack.unpackb(payload, raw=False)
    print(snap)
```

---

## Server Configuration

All options can be set via a YAML config file, environment variables, or CLI flags (CLI takes precedence).

| Env var | Flag | Default | Description |
|---|---|---|---|
| `PUB_ENDPOINT` | `--pub-endpoint` | `tcp://*:5555` | ZMQ market PUB socket |
| `ROUTER_ENDPOINT` | `--router-endpoint` | `tcp://*:5556` | ZMQ ROUTER socket |
| `DEPTH_LEVELS` | `--depth` | `20` | Orderbook depth per snapshot |
| `BINANCE_SYMBOLS` | `--binance` | *(none)* | Comma-separated Binance symbols |
| `OKX_SYMBOLS` | `--okx` | *(none)* | Comma-separated OKX symbols |
| `HYPERLIQUID_COINS` | `--hyperliquid` | *(none)* | Comma-separated Hyperliquid coins |
| `LOG_LEVEL` | `--log-level` | `info` | Tracing filter |
| `LOG_JSON` | `--log-json` | `false` | JSON log output |

```bash
cargo run --bin orderbook_server --release -- \
    --binance btcusdt,ethusdt \
    --okx BTC-USDT,ETH-USDT \
    --hyperliquid BTC,ETH \
    --depth 10 \
    --pub-endpoint tcp://*:5555 \
    --router-endpoint tcp://*:5556
```

---

## Storage & Replay

See [`docs/storage.md`](docs/storage.md) for full details on the MessagePack format, file layout, rotation, compression, and Python readers.

```yaml
storage:
  enabled: true
  base_path: "./data"
  depth: 20
  flush_interval: 1000    # ms
  rotation: "daily"       # "daily" | "none"
  zstd_level: 3           # 0 = disabled
```

```bash
python3 scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
python3 scripts/read_mpack.py data/binance/BTCUSDT/   # all dates
```

---

## Commands

```bash
# Build
cargo build --release
cargo build --bin orderbook_server --release

# Check / lint / format
cargo check
cargo fmt
cargo clippy

# Run server
cargo run --bin orderbook_server --release -- --config configs/server.yaml
cargo run --bin orderbook_server --release -- --binance btcusdt,ethusdt --depth 10

# Polymarket tools
cargo run --example polymarket_orderbook --release
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml

# Kalshi tools
cargo run --example kalshi_orderbook --release -- --config configs/kalshi.yaml

# Multi-exchange TUI example
cargo run --example orderbook

# ZMQ publisher example
cargo run --example zmq_publisher
```
