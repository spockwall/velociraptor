# velociraptor

A high-performance Rust library and server for real-time cryptocurrency orderbook streaming from major exchanges.

## Workspace

```
velociraptor/
├── libs/        # Shared constants and exchange endpoint definitions
└── orderbook/   # Core library + deployable server binary
```

## Supported Exchanges

| Exchange | Status | Stream Type |
|----------|--------|-------------|
| Binance  | Live   | Partial Book Depth 20 @ 100ms (`fstream.binance.com`) |
| OKX      | Live   | `books` channel — snapshot + incremental updates |

---

## Quick Start

```bash
# Build and run the server (streams Binance BTCUSDT + ETHUSDT by default)
cargo run --bin orderbook_server --release

# Custom symbols
BINANCE_SYMBOLS=btcusdt,solusdt OKX_SYMBOLS=BTC-USDT cargo run --bin orderbook_server --release

# Subscribe from Python
python3 orderbook/examples/zmq_subscriber.py --exchange binance --symbol BTCUSDT --type snapshot --interval 500
```

---

## Deploying the Server

### Build

```bash
cargo build --bin orderbook_server --release
# Binary at: target/release/orderbook_server
```

### Configuration

All options can be set via environment variables or CLI flags. CLI flags take precedence.

| Env var          | Flag              | Default           | Description                              |
|------------------|-------------------|-------------------|------------------------------------------|
| `PUB_ENDPOINT`   | `--pub-endpoint`  | `tcp://*:5555`    | ZMQ PUB socket — clients subscribe here |
| `ROUTER_ENDPOINT`| `--router-endpoint`| `tcp://*:5556`   | ZMQ ROUTER socket — clients send requests here |
| `DEPTH_LEVELS`   | `--depth`         | `20`              | Orderbook depth per published snapshot   |
| `BINANCE_SYMBOLS`| `--binance`       | `btcusdt,ethusdt` | Comma-separated Binance symbols          |
| `OKX_SYMBOLS`    | `--okx`           | *(none)*          | Comma-separated OKX SPOT symbols         |
| `LOG_LEVEL`      | `--log-level`     | `info`            | Tracing filter (e.g. `debug`, `orderbook=trace`) |
| `LOG_JSON`       | `--log-json`      | `false`           | JSON-formatted logs for log aggregators  |

A `.env` file in the working directory is loaded automatically if present.

### Example `.env`

```env
PUB_ENDPOINT=tcp://*:5555
ROUTER_ENDPOINT=tcp://*:5556
DEPTH_LEVELS=10
BINANCE_SYMBOLS=btcusdt,ethusdt,solusdt
OKX_SYMBOLS=BTC-USDT,ETH-USDT
LOG_LEVEL=info
LOG_JSON=true
```

### Run with flags

```bash
./orderbook_server \
    --binance btcusdt,ethusdt,solusdt \
    --okx BTC-USDT,ETH-USDT \
    --depth 10 \
    --pub-endpoint tcp://*:5555 \
    --router-endpoint tcp://*:5556

# Help
./orderbook_server --help
```

---

## Architecture

The system has three layers: **exchange connectors** feed raw wire data into the **orderbook engine**, which maintains live state and fans out events to **consumers**.

```
┌──────────────────────────────────────────────────────────────────┐
│                        OrderbookSystem                           │
│  (thin wiring layer — connects connectors to the engine)         │
│                                                                  │
│  ┌──────────────┐   mpsc::unbounded   ┌────────────────────────┐ │
│  │   Binance    │ ──────────────────► │                        │ │
│  │  Connector   │                     │   OrderbookEngine      │ │
│  └──────────────┘                     │                        │ │
│                                       │  DashMap<              │ │
│  ┌──────────────┐   mpsc::unbounded   │    "exchange:SYMBOL",  │ │
│  │     OKX      │ ──────────────────► │    Arc<Orderbook>      │ │
│  │  Connector   │                     │  >                     │ │
│  └──────────────┘                     │                        │ │
│                                       └──────────┬─────────────┘ │
└─────────────────────────────────────────────────│───────────────┘
                                                  │
                                   broadcast::channel
                                   (OrderbookEvent)
                                                  │
                   ┌──────────────────────────────┼──────────────────┐
                   │                              │                  │
                   ▼                              ▼                  ▼
          ┌────────────────┐           ┌──────────────────┐  ┌──────────────┐
          │  on_update()   │           │  ZmqPublisher    │  │ orderbooks() │
          │  async closure │           │  PUB + ROUTER    │  │ DashMap poll │
          └────────────────┘           └──────────────────┘  └──────────────┘
```

### Data Flow

```
Exchange WebSocket
  │
  ▼
ConnectionBase.handle_message()
  │  parse raw text
  ▼
MessageParser.parse_message()        (BinanceMessageParser / OkxMessageParser)
  │  returns Vec<OrderbookMessage>
  ▼
mpsc::UnboundedSender<OrderbookMessage>
  │  non-blocking send
  ▼
OrderbookEngine  (single async task)
  │
  ├─► broadcast  OrderbookEvent::RawUpdate(update)   ← emitted BEFORE apply
  │
  ├─► Orderbook.apply_update(update)                 ← mutates Arc<Orderbook>
  │     via Arc::make_mut (copy-on-write)
  │
  └─► broadcast  OrderbookEvent::Snapshot(snap)      ← emitted AFTER apply
        snap.book = Arc<Orderbook>  (full book state)
```

### Key Design Decisions

**`mpsc` ingest, `broadcast` output.** Multiple connectors push into a single unbounded channel (many-to-one). The engine fans out to multiple subscribers via a broadcast channel (one-to-many).

**`Arc<Orderbook>` in snapshots.** `OrderbookSnapshot.book` is an `Arc<Orderbook>`, not a copy. The engine uses `Arc::make_mut` (copy-on-write) so mutation only clones when a subscriber is holding an Arc reference.

**Two event types per update.** `RawUpdate` carries the raw wire data before apply — useful for logging or custom state machines. `Snapshot` carries the full book state after apply — used by ZmqPublisher.

**`OrderbookSystem` is a thin wrapper.** All state lives in `OrderbookEngine`. `OrderbookSystem` only wires connectors, starts tasks, and handles shutdown.

---

## Concurrency Model

```
tokio runtime
│
├── [task] BinanceConnector        — WebSocket I/O, heartbeat, reconnect
├── [task] OkxConnector            — WebSocket I/O, heartbeat, reconnect
├── [task] OrderbookEngine         — recv loop, apply updates, broadcast events
├── [task] on_update handler(s)    — one task per registered callback
└── [task] ZmqPublisher (optional) — reads broadcast, publishes over ZMQ
```

`SystemControl` holds two `AtomicBool` flags (`paused`, `shutdown`) shared across all tasks.

---

## ZMQ Interface

The server exposes two ZMQ sockets.

### PUB socket — `tcp://*:5555`

Publishes throttled orderbook data. Each message is a two-part frame: `[topic, json_payload]`.

**Topic format:** `"<exchange>:<SYMBOL>"` — e.g. `"binance:BTCUSDT"`

### ROUTER socket — `tcp://*:5556`

Clients connect a DEALER socket here to send subscription requests and receive acks.

```
Client (DEALER)                       Server (ROUTER)
  │                                          │
  │── { "action": "subscribe", ... } ───────►│
  │◄─ { "status": "ok", ... } ───────────────│
  │                                          │
  │── { "action": "add_channel", ... } ──────►│  ← starts a new WS connection
  │◄─ { "status": "ok", "message": "channel added" } ─│
```

### Subscription request (client → server)

```json
{
  "action":   "subscribe",
  "exchange": "binance",
  "symbol":   "BTCUSDT",
  "type":     "snapshot",
  "interval": 500
}
```

| Field      | Values                           | Notes                          |
|------------|----------------------------------|--------------------------------|
| `action`   | `subscribe` `unsubscribe` `add_channel` | |
| `exchange` | `binance` `okx`                  |                                |
| `symbol`   | e.g. `BTCUSDT`, `BTC-USDT`       | Exchange-native format         |
| `type`     | `snapshot` `bba`                 | Required for subscribe         |
| `interval` | milliseconds                     | Required for subscribe         |

### Data types

| `type`     | Published fields                                               |
|------------|---------------------------------------------------------------|
| `snapshot` | Full depth: `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread`, `mid`, `wmid`, `bids`, `asks` |
| `bba`      | Top of book only: `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread` |

### Adding a channel at runtime

Clients can request the server to start streaming a symbol that wasn't in the startup config:

```json
{ "action": "add_channel", "exchange": "binance", "symbol": "SOLUSDT" }
```

The server spins up a new WebSocket connection and acks immediately. Once data arrives, subscribe normally.

---

## Python Client

### Subscribe to live data

```bash
python3 orderbook/examples/zmq_subscriber.py \
    --exchange binance --symbol BTCUSDT \
    --type snapshot --interval 500

python3 orderbook/examples/zmq_subscriber.py \
    --exchange binance --symbol ETHUSDT \
    --type bba --interval 100
```

### Add a new channel at runtime

```bash
python3 orderbook/examples/zmq_subscriber.py \
    --add-channel --exchange binance --symbol SOLUSDT
```

### Minimal Python snippet

```python
import zmq, json

ctx = zmq.Context()

# Subscribe to data
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    topic, payload = sub.recv_multipart()
    snap = json.loads(payload)
    print(f"bid={snap['best_bid']}  ask={snap['best_ask']}  wmid={snap['wmid']:.4f}")
```

---

## Rust Consumer Patterns

### 1. Async callback — `on_update`

```rust
let _handle = system.on_update(|event| async move {
    if let OrderbookEvent::Snapshot(snap) = event {
        println!(
            "[{}:{}] bid={:.4} ask={:.4} wmid={:.4}",
            snap.exchange, snap.symbol,
            snap.book.best_bid().map(|(p, _)| p).unwrap_or(0.0),
            snap.book.best_ask().map(|(p, _)| p).unwrap_or(0.0),
            snap.book.wmid(),
        );
    }
});
system.run().await?;
```

### 2. Direct map poll — `orderbooks`

```rust
let books = system.orderbooks();

// in another task or timer:
if let Some(ob) = books.get("binance:BTCUSDT") {
    let (bids, asks) = ob.depth(5);
    println!("spread={:.4}", ob.spread().unwrap_or(0.0));
}
```

---

## Orderbook API

```rust
ob.best_bid()        // Option<(price, qty)>
ob.best_ask()        // Option<(price, qty)>
ob.spread()          // Option<f64>
ob.mid_price()       // Option<f64>
ob.depth(n)          // (Vec<(price, qty)>, Vec<(price, qty)>) — top-n bids and asks
ob.avg_bid_price(n)  // f64
ob.avg_ask_price(n)  // f64
ob.vamp(n)           // f64 — volume-weighted average mid price
ob.wmid()            // f64 — quantity-weighted mid price
```

---

## Subscription Builders

### Binance

```rust
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .build()
```

### OKX

```rust
OkxSubMsgBuilder::new()
    .with_orderbook_channel("BTC-USDT-SWAP", "SWAP")
    .with_orderbook_channel_multi(vec!["ETH-USDT", "SOL-USDT"], "SPOT")
    .build()
```

---

## Commands

```bash
# Build
cargo build --release
cargo build --bin orderbook_server --release

# Run server
cargo run --bin orderbook_server --release
cargo run --bin orderbook_server --release -- --binance btcusdt,solusdt --depth 10

# Run examples
cargo run --example stream_orderbook_instance
cargo run --example zmq_publisher
cargo run --example exchange_websocket

# Dev
cargo check
cargo test
cargo fmt
cargo clippy
```
