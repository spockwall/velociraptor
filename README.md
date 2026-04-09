# velociraptor

A high-performance Rust library and server for real-time cryptocurrency orderbook streaming from major exchanges.

## Supported Exchanges

| Exchange    | Status | Stream Type |
|-------------|--------|-------------|
| Binance     | Live   | Partial Book Depth 20 @ 100ms (`fstream.binance.com`) |
| OKX         | Live   | `books` channel — snapshot + incremental updates |
| Polymarket  | Live   | `book` snapshot + `price_change` incremental diffs |
| Hyperliquid | Live   | `l2Book` full snapshot on every update |

---

## Quick Start

**Step 1 — Edit the config file**

```toml
# configs/server.toml
[binance]
enabled = true
symbols = ["btcusdt", "ethusdt"]

[hyperliquid]
enabled = true
coins   = ["BTC", "ETH"]

# Polymarket: token IDs are resolved automatically from the Gamma API at startup.
# Specify the slug and window size; no manual token IDs needed.
[[polymarket]]
enabled       = true
slug          = "btc-updown-5m"
interval_secs = 300
```

**Step 2 — Start the server**

```bash
cargo run --bin orderbook_server --release -- --config configs/server.toml
```

**Step 3 — Subscribe from Python**

```bash
# Binance snapshot
python3 orderbook/examples/zmq_subscriber.py --exchange binance --symbol BTCUSDT --type snapshot --interval 500

# Polymarket snapshot (use token ID as symbol; printed in server logs at startup)
python3 orderbook/examples/zmq_subscriber.py \
    --exchange polymarket \
    --symbol 75467129615908319583031474642658885479135630431889036121812713428992454630178 \
    --type snapshot --interval 500
```

**Step 4 — Read stored snapshots**

```bash
python3 scripts/read_mpack.py data/binance/BTCUSDT/
python3 scripts/read_mpack.py data/polymarket/btc-updown-5m/2026-04-05/
```

---

## Polymarket Tools

### Live terminal visualiser

```bash
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.toml
# or
cargo run --example polymarket_orderbook --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --slug eth-updown-5m --interval-secs 300
```

Shows a live terminal UI with bids, asks, spread and mid for each market side. Rotates automatically to the next window as windows expire.

### Disk recorder

```bash
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.toml
# or
cargo run --bin polymarket_recorder --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --base-path ./data --depth 10 --zstd-level 3
```

Writes every orderbook snapshot to disk (at the WebSocket update rate, not the render rate). One file per window per side:

```
data/polymarket/btc-updown-5m/2026-04-05/09:55-10:00-up.mpack
                                         09:55-10:00-down.mpack
```

Files are optionally zstd-compressed after each window closes. See `docs/polymarket.md` for the full file format and Python reader.

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
| `BINANCE_SYMBOLS`   | `--binance`       | `btcusdt,ethusdt` | Comma-separated Binance symbols          |
| `OKX_SYMBOLS`       | `--okx`           | *(none)*          | Comma-separated OKX SPOT symbols         |
| `POLYMARKET_ASSETS` | `--polymarket`    | *(none)*          | Comma-separated Polymarket token IDs     |
| `HYPERLIQUID_COINS` | `--hyperliquid`   | *(none)*          | Comma-separated Hyperliquid coin names (e.g. `BTC,ETH`) |
| `LOG_LEVEL`         | `--log-level`     | `info`            | Tracing filter (e.g. `debug`, `orderbook=trace`) |
| `LOG_JSON`          | `--log-json`      | `false`           | JSON-formatted logs for log aggregators  |

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

# Help
./orderbook_server --help
```

### Run as systemd Services (Linux)

For unattended, long-running production deployments use the provided systemd units.

**Quick start:**

```bash
# 1. Edit User= and all absolute paths in each .service file to match your host
#    (search for /Users/spockwall and replace with your home/repo path)

# 2. Build release binaries
cargo build --bin orderbook_server --release
cargo build --bin polymarket_recorder --release

# 3. Install and enable
sudo cp systemd/orderbook-server.service    /etc/systemd/system/
sudo cp systemd/polymarket-recorder.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now orderbook-server
sudo systemctl enable --now polymarket-recorder

# 4. Check status and follow logs
sudo systemctl status orderbook-server
sudo journalctl -u orderbook-server -f

sudo systemctl status polymarket-recorder
sudo journalctl -u polymarket-recorder -f
```

Both services use `Restart=always` with a 10-second back-off, so they recover automatically from crashes, OOM kills, or network drops. See [`docs/systemd.md`](docs/systemd.md) for the full reference — restart policy details, log commands, disk-space monitoring for the recorder, and how to roll out a binary update.

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
│                                       │                        │ │
│  ┌──────────────┐   mpsc::unbounded   │                        │ │
│  │ Polymarket   │ ──────────────────► │                        │ │
│  │  Connector   │                     │                        │ │
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
├── [task] PolymarketConnector     — WebSocket I/O, reconnect (no heartbeat)
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

| Field      | Values                                  | Notes                          |
|------------|-----------------------------------------|--------------------------------|
| `action`   | `subscribe` `unsubscribe` `add_channel` |                                |
| `exchange` | `binance` `okx` `polymarket` `hyperliquid` |                             |
| `symbol`   | e.g. `BTCUSDT`, `BTC-USDT`, `<token_id>` | Exchange-native format       |
| `type`     | `snapshot` `bba`                        | Required for subscribe         |
| `interval` | milliseconds                            | Required for subscribe         |

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

python3 orderbook/examples/zmq_subscriber.py \
    --add-channel --exchange polymarket --symbol 71321045...
```

### Minimal Python snippet — Binance

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

### Minimal Python snippet — Polymarket

Subscribe to a Yes or No token by using its token ID as the symbol. The topic format is `polymarket:<token_id>`.

```python
import zmq, json

YES_TOKEN = "75467129615908319583031474642658885479135630431889036121812713428992454630178"

ctx = zmq.Context()

# Request a snapshot subscription via ROUTER socket
dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({
    "action":   "subscribe",
    "exchange": "polymarket",
    "symbol":   YES_TOKEN,
    "type":     "snapshot",
    "interval": 500,
})
ack = dealer.recv_json()
print("ack:", ack)

# Receive published snapshots
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

### Polymarket

```rust
PolymarketSubMsgBuilder::new()
    .with_asset("71321045679252212594626385532706912750332728571942532289631379312455583992563")
    .with_asset("52114319501245915516055106046884209969926127482827954674443846427813813222426")
    .build()
```

Token IDs can be fetched from the Polymarket API — see `scripts/fetch_polymarket_tokens.py`.

### Hyperliquid

```rust
HyperliquidSubMsgBuilder::new()
    .with_coin("BTC")
    .with_coin("ETH")
    .build()

// or multiple at once:
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
cargo run --bin orderbook_server --release
cargo run --bin orderbook_server --release -- --binance btcusdt,solusdt --depth 10

# Run examples
cargo run --example stream_orderbook_instance
cargo run --example zmq_publisher
cargo run --example exchange_websocket

# Polymarket tools
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.toml
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.toml

# Dev
cargo check
cargo test
cargo fmt
cargo clippy
```
