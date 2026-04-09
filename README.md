# velociraptor

A high-performance Rust library and server for real-time cryptocurrency orderbook streaming from major exchanges.

| Exchange    | Status | Stream Type |
|-------------|--------|-------------|
| Binance     | Live   | Partial Book Depth 20 @ 100ms |
| OKX         | Live   | `books` channel — snapshot + incremental updates |
| Polymarket  | Live   | `book` snapshot + `price_change` incremental diffs |
| Hyperliquid | Live   | `l2Book` full snapshot on every update |

---

## Quick Start

### 1. Configure

Edit `configs/server.toml` to enable the exchanges you want:

```toml
[binance]
enabled = true
symbols = ["btcusdt", "ethusdt"]

[hyperliquid]
enabled = true
coins   = ["BTC", "ETH"]

# Polymarket token IDs are resolved automatically from the Gamma API.
[[polymarket]]
enabled       = true
slug          = "btc-updown-5m"
interval_secs = 300
```

### 2. Start the server

```bash
cargo run --bin orderbook_server --release -- --config configs/server.toml
```

### 3. Subscribe from Python

```bash
# Binance
python3 orderbook/examples/zmq_subscriber.py \
    --exchange binance --symbol BTCUSDT --type snapshot --interval 500

# Polymarket (token ID is printed in server logs at startup)
python3 orderbook/examples/zmq_subscriber.py \
    --exchange polymarket \
    --symbol 75467129615908319583031474642658885479135630431889036121812713428992454630178 \
    --type snapshot --interval 500
```

### 4. Read stored snapshots

```bash
python3 scripts/read_mpack.py data/binance/BTCUSDT/
python3 scripts/read_mpack.py data/polymarket/btc-updown-5m/2026-04-05/
```

---

## Running as systemd Services (Linux)

For unattended, long-running deployments. Both services use `Restart=always` with a 10 s back-off and auto-start on boot.

**Before installing**, open each `.service` file under `systemd/` and replace `/Users/spockwall` with your actual home and repo paths.

```bash
# 1. Build release binaries
cargo build --bin orderbook_server --release
cargo build --bin polymarket_recorder --release

# 2. Install
sudo cp systemd/orderbook-server.service    /etc/systemd/system/
sudo cp systemd/polymarket-recorder.service /etc/systemd/system/
sudo systemctl daemon-reload

# 3. Enable and start
sudo systemctl enable --now orderbook-server
sudo systemctl enable --now polymarket-recorder

# 4. Check status / logs
sudo systemctl status orderbook-server
sudo journalctl -u orderbook-server -f

sudo systemctl status polymarket-recorder
sudo journalctl -u polymarket-recorder -f
```

See [`docs/systemd.md`](docs/systemd.md) for restart policy details, log commands, disk-space monitoring, and binary update procedure.

---

## Polymarket Tools

### Live terminal visualiser

Shows a live terminal UI with bids, asks, spread, and mid. Rotates to the next window automatically.

```bash
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.toml
```

### Disk recorder

Writes every orderbook snapshot to disk at the WebSocket update rate. One file per window per side:

```
data/polymarket/btc-updown-5m/2026-04-05/09:55-10:00-up.mpack
                                         09:55-10:00-down.mpack
```

```bash
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.toml
```

Files are optionally zstd-compressed after each window closes. See [`docs/polymarket.md`](docs/polymarket.md) for the full file format and Python reader.

---

## Deploying the Server

### Build

```bash
cargo build --bin orderbook_server --release
# Binary at: target/release/orderbook_server
```

### Configuration

All options can be set via environment variables or CLI flags (flags take precedence).

| Env var             | Flag               | Default           | Description                                      |
|---------------------|--------------------|-------------------|--------------------------------------------------|
| `PUB_ENDPOINT`      | `--pub-endpoint`   | `tcp://*:5555`    | ZMQ PUB socket — clients subscribe here          |
| `ROUTER_ENDPOINT`   | `--router-endpoint`| `tcp://*:5556`    | ZMQ ROUTER socket — clients send requests here   |
| `DEPTH_LEVELS`      | `--depth`          | `20`              | Orderbook depth per published snapshot           |
| `BINANCE_SYMBOLS`   | `--binance`        | `btcusdt,ethusdt` | Comma-separated Binance symbols                  |
| `OKX_SYMBOLS`       | `--okx`            | *(none)*          | Comma-separated OKX SPOT symbols                 |
| `POLYMARKET_ASSETS` | `--polymarket`     | *(none)*          | Comma-separated Polymarket token IDs             |
| `HYPERLIQUID_COINS` | `--hyperliquid`    | *(none)*          | Comma-separated Hyperliquid coin names           |
| `LOG_LEVEL`         | `--log-level`      | `info`            | Tracing filter (e.g. `debug`, `orderbook=trace`) |
| `LOG_JSON`          | `--log-json`       | `false`           | JSON-formatted logs for log aggregators          |

A `.env` file in the working directory is loaded automatically if present.

### Example `.env`

```env
PUB_ENDPOINT=tcp://*:5555
ROUTER_ENDPOINT=tcp://*:5556
DEPTH_LEVELS=10
BINANCE_SYMBOLS=btcusdt,ethusdt,solusdt
OKX_SYMBOLS=BTC-USDT,ETH-USDT
POLYMARKET_ASSETS=71321045679252212594626385532706912750332728571942532289631379312455583992563
HYPERLIQUID_COINS=BTC,ETH
LOG_LEVEL=info
LOG_JSON=true
```

### Run with flags

```bash
./orderbook_server \
    --binance btcusdt,ethusdt,solusdt \
    --okx BTC-USDT,ETH-USDT \
    --polymarket 71321045...,52114319... \
    --hyperliquid BTC,ETH,SOL \
    --depth 10 \
    --pub-endpoint tcp://*:5555 \
    --router-endpoint tcp://*:5556

./orderbook_server --help
```

---

## Storage & Replay

Enable storage in `configs/server.toml`:

```toml
[storage]
enabled        = true
base_path      = "./data"
depth          = 20
flush_interval = 1000    # ms between flushes
rotation       = "daily" # "daily" | "none"
```

Snapshots are written as append-only MessagePack files:

```
data/{exchange}/{symbol}/{YYYY-MM-DD}.mpack
```

Read them back in Python:

```bash
pip install msgpack pandas
python3 scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
python3 scripts/read_mpack.py data/binance/BTCUSDT/   # all dates for that symbol
```

---

## ZMQ Interface

The server exposes two ZMQ sockets.

### PUB socket — `tcp://*:5555`

Publishes throttled orderbook data as two-part frames: `[topic, json_payload]`.

Topic format: `"<exchange>:<SYMBOL>"` — e.g. `"binance:BTCUSDT"`

### ROUTER socket — `tcp://*:5556`

Clients connect a DEALER socket here to send subscription requests and receive acks.

```
Client (DEALER)                       Server (ROUTER)
  │── { "action": "subscribe", ... } ───────►│
  │◄─ { "status": "ok", ... } ───────────────│
  │── { "action": "add_channel", ... } ──────►│
  │◄─ { "status": "ok", "message": "channel added" } ──│
```

### Subscription request

```json
{
  "action":   "subscribe",
  "exchange": "binance",
  "symbol":   "BTCUSDT",
  "type":     "snapshot",
  "interval": 500
}
```

| Field      | Values                                     | Notes                    |
|------------|--------------------------------------------|--------------------------|
| `action`   | `subscribe` `unsubscribe` `add_channel`    |                          |
| `exchange` | `binance` `okx` `polymarket` `hyperliquid` |                          |
| `symbol`   | e.g. `BTCUSDT`, `BTC-USDT`, `<token_id>`  | Exchange-native format   |
| `type`     | `snapshot` `bba`                           | Required for subscribe   |
| `interval` | milliseconds                               | Required for subscribe   |

### Data types

| `type`     | Published fields |
|------------|-----------------|
| `snapshot` | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread`, `mid`, `wmid`, `bids`, `asks` |
| `bba`      | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread` |

### Adding a channel at runtime

```json
{ "action": "add_channel", "exchange": "binance", "symbol": "SOLUSDT" }
```

The server spins up a new WebSocket connection and acks immediately. Once data arrives, subscribe normally.

---

## Python Client

### Subscribe to live data

```bash
python3 orderbook/examples/zmq_subscriber.py \
    --exchange binance --symbol BTCUSDT --type snapshot --interval 500

python3 orderbook/examples/zmq_subscriber.py \
    --exchange binance --symbol ETHUSDT --type bba --interval 100
```

### Add a channel at runtime

```bash
python3 orderbook/examples/zmq_subscriber.py --add-channel --exchange binance --symbol SOLUSDT
python3 orderbook/examples/zmq_subscriber.py --add-channel --exchange polymarket --symbol 71321045...
```

### Minimal snippet — Binance

```python
import zmq, json

ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    topic, payload = sub.recv_multipart()
    snap = json.loads(payload)
    print(f"bid={snap['best_bid']}  ask={snap['best_ask']}  wmid={snap['wmid']:.4f}")
```

### Minimal snippet — Polymarket

Topic format is `polymarket:<token_id>`. Use the token ID as the symbol.

```python
import zmq, json

YES_TOKEN = "75467129615908319583031474642658885479135630431889036121812713428992454630178"

ctx = zmq.Context()

dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({
    "action": "subscribe", "exchange": "polymarket",
    "symbol": YES_TOKEN, "type": "snapshot", "interval": 500,
})
print("ack:", dealer.recv_json())

sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, f"polymarket:{YES_TOKEN}".encode())

while True:
    topic, payload = sub.recv_multipart()
    snap = json.loads(payload)
    print(f"bid={snap['best_bid']}  ask={snap['best_ask']}  spread={snap['spread']:.4f}")
```

Find Polymarket token IDs with:

```bash
python3 scripts/fetch_polymarket_tokens.py --search "bitcoin"
```

---

## Architecture

Three layers: **exchange connectors** feed raw wire data into the **orderbook engine**, which maintains live state and fans out events to **consumers**.

```
┌──────────────────────────────────────────────────────────────────┐
│                        OrderbookSystem                           │
│  ┌──────────────┐   mpsc::unbounded   ┌────────────────────────┐ │
│  │   Binance    │ ──────────────────► │                        │ │
│  │  Connector   │                     │   OrderbookEngine      │ │
│  └──────────────┘                     │                        │ │
│                                       │  DashMap<              │ │
│  ┌──────────────┐   mpsc::unbounded   │    "exchange:SYMBOL",  │ │
│  │     OKX      │ ──────────────────► │    Arc<Orderbook>      │ │
│  │  Connector   │                     │  >                     │ │
│  └──────────────┘                     │                        │ │
│                                       │                        │ │
│  ┌──────────────┐   mpsc::unbounded   │                        │ │
│  │ Polymarket   │ ──────────────────► │                        │ │
│  │  Connector   │                     │                        │ │
│  └──────────────┘                     └──────────┬─────────────┘ │
└─────────────────────────────────────────────────│───────────────┘
                                                  │ broadcast::channel
                   ┌──────────────────────────────┼──────────────────┐
                   ▼                              ▼                  ▼
          ┌────────────────┐           ┌──────────────────┐  ┌──────────────┐
          │  on_update()   │           │  ZmqPublisher    │  │ orderbooks() │
          │  async closure │           │  PUB + ROUTER    │  │ DashMap poll │
          └────────────────┘           └──────────────────┘  └──────────────┘
```

### Data Flow

```
Exchange WebSocket
  → ConnectionBase.handle_message()   parse raw text
  → MessageParser.parse_message()     returns Vec<OrderbookMessage>
  → mpsc channel
  → OrderbookEngine
      ├─► broadcast RawUpdate(update)          (before apply)
      ├─► Orderbook.apply_update(update)       (Arc::make_mut copy-on-write)
      └─► broadcast Snapshot(snap)             (after apply)
```

### Key Design Decisions

**`mpsc` ingest, `broadcast` output.** Multiple connectors push into a single unbounded channel; the engine fans out to multiple subscribers via broadcast.

**`Arc<Orderbook>` in snapshots.** Copy-on-write via `Arc::make_mut` — mutation only clones when a subscriber holds a reference.

**Two event types per update.** `RawUpdate` carries wire data before apply (useful for logging/custom state machines). `Snapshot` carries full book state after apply (used by ZmqPublisher).

### Concurrency Model

```
tokio runtime
├── [task] BinanceConnector        — WebSocket I/O, heartbeat, reconnect
├── [task] OkxConnector            — WebSocket I/O, heartbeat, reconnect
├── [task] PolymarketConnector     — WebSocket I/O, reconnect
├── [task] OrderbookEngine         — recv loop, apply updates, broadcast events
├── [task] on_update handler(s)    — one task per registered callback
└── [task] ZmqPublisher (optional) — reads broadcast, publishes over ZMQ
```

`SystemControl` holds two `AtomicBool` flags (`paused`, `shutdown`) shared across all tasks.

---

## Rust Consumer Patterns

### Async callback — `on_update`

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

### Direct map poll — `orderbooks`

```rust
let books = system.orderbooks();

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

### Polymarket

```rust
PolymarketSubMsgBuilder::new()
    .with_asset("71321045679252212594626385532706912750332728571942532289631379312455583992563")
    .with_asset("52114319501245915516055106046884209969926127482827954674443846427813813222426")
    .build()
```

Token IDs can be fetched with `python3 scripts/fetch_polymarket_tokens.py`.

### Hyperliquid

```rust
HyperliquidSubMsgBuilder::new()
    .with_coins(&["BTC", "ETH", "SOL"])
    .build()
```

Symbols are uppercase coin names only — no quote currency (e.g. `"BTC"` not `"BTCUSDT"`).

---

## Commands

```bash
# Build
cargo build --release
cargo build --bin orderbook_server --release

# Run server
cargo run --bin orderbook_server --release -- --config configs/server.toml
cargo run --bin orderbook_server --release -- --binance btcusdt,solusdt --depth 10

# Polymarket tools
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.toml
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.toml

# Examples
cargo run --example stream_orderbook_instance
cargo run --example zmq_publisher
cargo run --example exchange_websocket

# Dev
cargo check
cargo test
cargo fmt
cargo clippy
```
