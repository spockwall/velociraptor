# velociraptor

A high-performance Rust workspace for real-time market data streaming and order execution on prediction markets and crypto exchanges.

| Exchange       | Status | Stream Type |
|----------------|--------|-------------|
| Binance (USDT-M futures) | Live | Partial Book Depth 20 @ 100ms |
| Binance Spot   | Live   | Partial Book Depth 20 @ 100ms + raw `@trade` stream |
| OKX            | Live   | `books` channel — snapshot + incremental updates |
| Polymarket     | Live   | `book` snapshot + `price_change` incremental diffs |
| Hyperliquid    | Live   | `l2Book` full snapshot on every update |
| Kalshi         | Live   | `orderbook_snapshot` + signed `orderbook_delta` — requires API key |

---

## Workspace

| Crate | Purpose |
|---|---|
| `libs` | Shared protocol types, configs, credentials, Redis client, terminal UI |
| `orderbook` | Multi-exchange market data engine — `StreamEngine`, `StreamSystem`, connectors |
| `zmq_server` | ZMQ transport layer — PUB (market data), ROUTER (subscriptions), PUB (user events) |
| `recorder` | Append-only MessagePack snapshot writer with daily rotation and zstd compression |
| `executor` | ZMQ REP gateway for REST order placement (stub) |
| `backend` | Axum HTTP API — reads Redis, exposes market data over REST |

---
## Command Overview 

```bash
# Build
cargo build --release

# Check / lint / format
cargo check --workspace
cargo fmt
cargo clippy

# Run server (ZMQ + optional Redis)
cargo run --bin orderbook_server --release -- --config configs/server.yaml

# Run HTTP backend (reads Redis)
cargo run --bin backend --release -- --config configs/server.yaml

# Terminal visualisers
cargo run --example polymarket_orderbook --release
cargo run --example kalshi_orderbook --release
cargo run --example orderbook --release          # multi-exchange TUI

# Disk recorder (Polymarket)
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml

# Disk recorder (Binance, Binance Spot, Okx, yperliquid)
cargo run --bin orderbook_recorder --release -- --config configs/server.yaml

# Price-to-beat archive (Polymarket + Kalshi target prices → CSV)
cargo run --bin price_to_beat_fetcher --release -- --config configs/example.yaml
cargo run --bin price_to_beat_backfill --release -- polymarket \
    --base-slug btc-updown-15m --interval-secs 900 --from 2026-04-25T00:00:00Z

# Asset-id archive (Polymarket market_id + yes/no token IDs → CSV)
cargo run --bin asset_id_fetcher --release -- --config configs/example.yaml

# Python subscriber
python3 zmq_server/examples/orderbook_subscriber.py

# Run the system via docker
# !/usr/bin/env bash
# Boot the full stack in production mode (everything in containers).
#   redis            -> 127.0.0.1:6379
#   backend          -> 127.0.0.1:3000
#   orderbook_server -> 127.0.0.1:5555 (PUB) / :5556 (ROUTER)
#   frontend (nginx) -> 127.0.0.1:8080
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d --build

# Run the system via command line
docker compose up -d redis
cargo run --bin orderbook_server --release -- --config configs/example.yaml
cargo run --bin backend --release -- --config configs/example.yaml
cd frontend && npm run dev
```

## Quick Start

### 1. Configure

Copy and edit the example config:

```bash
cp configs/example.yaml configs/server.yaml
```

`configs/server.yaml` controls every exchange and storage option:

```yaml
server:
  pub_endpoint: "tcp://*:5555"
  router_endpoint: "tcp://*:5556"

redis:
  enabled: true
  url: "redis://127.0.0.1:6379"
  snapshot_cap: 100     # max snapshots kept per symbol
  trade_cap: 1000         # max trades kept per symbol
  event_list_cap: 5000   # max entries in events:fills / events:orders

backend:
  port: 3000

storage:
  enabled: false
  base_path: "./data"
  depth: 20
  flush_interval: 1000   # ms
  rotation: "daily"      # "daily" | "none"
  zstd_level: 0          # 0 = off, 1–22 = zstd level

binance:                 # USDT-margined futures (fstream.binance.com)
  enabled: true
  symbols: ["btcusdt", "ethusdt"]

binance_spot:            # spot (stream.binance.com) — depth + trades
  enabled: true
  symbols: ["btcusdt", "ethusdt"]

okx:
  enabled: true
  symbols: ["BTC-USDT", "ETH-USDT-SWAP"]

hyperliquid:
  enabled: true
  coins: ["BTC", "ETH"]

kalshi:
  market:
    - enable: true
      series: "KXBTC15M"   # 15-min BTC markets, auto-rotates
    - enable: true
      series: "KXETH15M"

polymarket:
  markets:
    - enabled: true
      slug: "btc-updown-15m"
      interval_secs: 900   # rolling-window length
    - enabled: true
      slug: "eth-updown-15m"
      interval_secs: 900
```

For Kalshi, place your RSA credentials in `credentials/kalshi.yaml`:

```yaml
key_id: "<your-key-id-uuid>"
private_key: |
  -----BEGIN PRIVATE KEY-----
  <your-rsa-private-key-pem>
  -----END PRIVATE KEY-----
ws_url: "wss://api.elections.kalshi.com/trade-api/ws/v2"
```

### 2. Start Redis (optional, required for backend)

```bash
docker compose up redis -d
```

### 3. Start the ZMQ server

```bash
cargo run --bin orderbook_server --release -- --config configs/server.yaml
```

The server logs its socket addresses on startup:

```
ZMQ PUB    tcp://*:5555
ZMQ ROUTER tcp://*:5556
ZMQ user PUB  ipc:///tmp/trading/ws_status.sock
Redis integration enabled (snapshot_cap=1000, trade_cap=500)
```

### 4. Start the HTTP backend (optional)

```bash
cargo run --bin backend --release -- --config configs/server.yaml
```

Query market data from Redis over HTTP:

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/bba/binance/btcusdt
curl http://localhost:3000/api/orderbook/binance/btcusdt
curl "http://localhost:3000/api/snapshots/binance/btcusdt?limit=10"
curl "http://localhost:3000/api/trades/binance_spot/btcusdt?limit=5"
curl "http://localhost:3000/api/trades/polymarket/<asset_id>?limit=5"
```

All responses are JSON. Returns `404` when a key has no data yet, `500` on decode error.

### 5. Subscribe from Python

```bash
pip install pyzmq msgpack

# Binance BTC snapshot (default)
python3 zmq_server/examples/orderbook_subscriber.py

# OKX ETH, BBA type
python3 zmq_server/examples/orderbook_subscriber.py \
    --exchange okx --symbol ETH-USDT-SWAP --type bba

# Hyperliquid
python3 zmq_server/examples/orderbook_subscriber.py \
    --exchange hyperliquid --symbol BTC

# Polymarket (token ID as symbol)
python3 zmq_server/examples/orderbook_subscriber.py \
    --exchange polymarket \
    --symbol 71321045679252212594626385532706912750332728571942532289631379312455583992563
```

### 6. Read stored snapshots

```bash
python3 scripts/read_mpack.py data/binance/BTCUSDT/
python3 scripts/read_mpack.py data/binance_spot/btcusdt/                       # snapshots + trades
python3 scripts/read_mpack.py data/binance_spot/btcusdt/2026-04-25-trades.mpack
python3 scripts/read_mpack.py data/polymarket/btc-updown-15m/2026-04-05/
```

---

## ZMQ Quick Start

This section shows the minimal code to connect to a running `orderbook_server`
and receive live data, subscribe to user events, and send an order.

### Receive market snapshots (Python)

Two sockets are needed: a **DEALER** for the subscribe handshake and a **SUB**
for the data stream.

```python
import zmq, msgpack

ctx = zmq.Context()

# 1. Send subscribe request over DEALER → ROUTER
dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")          # router_endpoint
dealer.send_json({
    "action":   "subscribe",
    "exchange": "binance",
    "symbol":   "BTCUSDT",
    "type":     "snapshot",                     # or "bba"
})
ack = dealer.recv_json()
assert ack["status"] == "ok", ack

# 2. Receive snapshots on SUB socket
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")             # pub_endpoint (MARKET_DATA_SOCKET)
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    _topic, payload = sub.recv_multipart()
    snap = msgpack.unpackb(payload, raw=False)
    exchange = next(iter(snap["exchange"]))      # {"binance": 0} → "binance"
    print(f"[{exchange}:{snap['symbol']}]  bid={snap['best_bid']}  ask={snap['best_ask']}  wmid={snap['wmid']:.4f}")
```

**Use the full subscriber script** for all exchanges and BBA mode:

```bash
pip install pyzmq msgpack

python3 zmq_server/examples/orderbook_subscriber.py                         # Binance BTC snapshot
python3 zmq_server/examples/orderbook_subscriber.py --exchange okx --symbol BTC-USDT --type bba
python3 zmq_server/examples/orderbook_subscriber.py --exchange hyperliquid --symbol BTC
python3 zmq_server/examples/orderbook_subscriber.py --exchange kalshi --symbol KXBTC15M-26APR130415-15
python3 zmq_server/examples/orderbook_subscriber.py \
    --exchange polymarket \
    --symbol 71321045679252212594626385532706912750332728571942532289631379312455583992563
```

> **`exchange` encoding:** `rmp-serde` encodes Rust enum variants as a
> single-key msgpack map — `{"Binance": 0}`. Extract the name with
> `next(iter(snap["exchange"]))`.

### Receive user events (Python)

No handshake needed — connect directly to the user PUB socket and set a topic
filter.

```python
import zmq, msgpack

ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("ipc:///tmp/trading/ws_status.sock")   # WS_STATUS_SOCKET

# Filter to one exchange, or b"user." for all
sub.setsockopt(zmq.SUBSCRIBE, b"user.polymarket.")

while True:
    topic, payload = sub.recv_multipart()
    ev = msgpack.unpackb(payload, raw=False)
    kind = ev.get("type")
    if kind == "fill":
        print(f"FILL  {ev['side']} {ev['qty']} @ {ev['px']}  fee={ev['fee']}")
    elif kind == "order_update":
        print(f"ORDER {ev['status']}  oid={ev['exchange_oid']}")
```

### Send an order (Python)

Connect a REQ socket to the executor. All frames are msgpack.

```python
import zmq, msgpack

ctx = zmq.Context()
req = ctx.socket(zmq.REQ)
req.connect("ipc:///tmp/trading/executor_orders.sock")   # EXECUTOR_ORDER_SOCKET

order = {
    "req_id":   1,
    "exchange": "polymarket",
    "action":   "place",
    "client_oid": "my-order-001",
    "symbol":   "<token_id>",
    "side":     "buy",
    "kind":     "limit",
    "px":       0.55,
    "qty":      10.0,
    "tif":      "GTC",
}
req.send(msgpack.packb(order, use_bin_type=True))
response = msgpack.unpackb(req.recv(), raw=False)
print(response)   # {"req_id": 1, "result": {"result": "ack", ...}}
```

### Unsubscribe

Send an unsubscribe request on the same DEALER socket before closing:

```python
dealer.send_json({"action": "unsubscribe", "exchange": "binance", "symbol": "BTCUSDT"})
```

See [`docs/protocol.md`](docs/protocol.md) for all socket addresses, topic
formats, payload schemas, and `OrderAction` variants.

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

Config: `configs/polymarket.yaml`

### Disk recorder

Writes every orderbook snapshot and last-trade event per window per side:

```
data/polymarket/btc-updown-15m/2026-04-05/
    09:45-10:00-up.mpack
    09:45-10:00-down.mpack
    09:45-10:00-up-trades.mpack
    09:45-10:00-down-trades.mpack
```

Snapshot files contain `[price, qty]` depth levels. Trade files contain one record per matched maker-taker event (`ts_ns`, `price`, `size`, `side`, `fee_rate_bps`). Up and Down tokens are always written to separate files.

```bash
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml
```

See [`docs/storage.md`](docs/storage.md) for file format, schemas, and Python readers.

---

## Kalshi Tools

Kalshi is a CFTC-regulated prediction market. Authentication is required even for market data.

```bash
cp credentials/example.yaml credentials/kalshi.yaml
# fill in key_id / private_key / ws_url

cargo run --example kalshi_orderbook --release
```

Config: `configs/kalshi.yaml`. The scheduler auto-rotates to the next 15-minute window.

See [`docs/kalshi.md`](docs/kalshi.md) for ticker format and scheduler details.

---

## Price-to-Beat Archive

Two binaries archive Polymarket / Kalshi target prices (`priceToBeat` /
`finalPrice`, or Kalshi `floor_strike` / `expiration_value`) to per-day CSV
files at `data/price_to_beat/{exchange}/{base_slug_or_series}/{YYYY-MM-DD}.csv`.

### `price_to_beat_fetcher` — long-running daemon

Reads `configs/example.yaml`, drives one task per enabled market.
**Auto-backfills on startup** from the latest CSV row (or `--seed-from` if the
archive is empty), then polls every interval boundary forever. The CSV is the
source of truth; restarting fills any gaps automatically.

```bash
cargo run --release --bin price_to_beat_fetcher -- \
    --config configs/example.yaml
```

Defaults: `--lookback-secs 3600` (target windows that closed ≥1h ago, so the
oracle has reported), `--seed-from 2026-04-10T00:00:00Z` (only used when the
archive is empty), `--archive-dir ./data/price_to_beat`.

### `price_to_beat_backfill` — one-shot

Walks one market over a user-specified `--from` / `--to` range. Useful for
filling gaps in a single market without restarting the daemon. Idempotent
(dedup by `window_start`).

```bash
# Polymarket 15-min
cargo run --release --bin price_to_beat_backfill -- polymarket \
    --base-slug btc-updown-15m --interval-secs 900 \
    --from 2026-04-25T00:00:00Z

# Polymarket 5-min
cargo run --release --bin price_to_beat_backfill -- polymarket \
    --base-slug btc-updown-5m --interval-secs 300 \
    --from 2026-04-25T00:00:00Z

# Kalshi 15-min series
cargo run --release --bin price_to_beat_backfill -- kalshi \
    --series KXBTC15M --interval-secs 900 \
    --from 2026-04-25T00:00:00Z
```

Defaults: `--to` is now, `--archive-dir` is `./data/price_to_beat`,
`--interval-secs` is `900`. Inter-request spacing is a compile-time const
(`REQUEST_SPACING_MS = 100` in `orderbook/src/price_to_beat.rs`).

CSV columns: `ts_recorded, exchange, base_slug, full_slug, window_start,
window_end, price_to_beat, final_price, direction`.

See [`docs/storage.md`](docs/storage.md#price-to-beat-archive-polymarket--kalshi)
for the full schema, why the 1-hour lookback (the oracle reports late), and
Python read recipes.

---

## Asset-ID Archive

`asset_id_fetcher` is a long-running daemon that records Polymarket market
identifiers — `market_id`, `yes_asset_id`, `no_asset_id` — for every rolling
window. Useful for joining historical orderbook / price-to-beat data with the
exact asset IDs the engine traded on. Kalshi markets are skipped (single
ticker, no yes/no token pair).

**Auto-backfills on startup** from the latest CSV row (or `--seed-from` if the
archive is empty), then polls every interval boundary forever. The CSV is the
source of truth; restarting fills any gaps automatically.

```bash
cargo run --bin asset_id_fetcher --release -- \
    --config configs/example.yaml \
    --seed-from 2026-04-10T00:00:00Z \
    --archive-dir ./data/asset_ids
```

Defaults: `--seed-from 2026-04-10T00:00:00Z` (only used when the archive is
empty for a market), `--archive-dir ./data/asset_ids`,
`--http-timeout-secs 8`. Inter-request spacing is a 100ms compile-time
constant in `orderbook/src/bin/asset_id_fetcher.rs`.

Output layout — one CSV per UTC day per `base_slug`:

```
data/asset_ids/polymarket/{base_slug}/{YYYY-MM-DD}.csv
```

CSV columns:

```
ts_recorded, base_slug, full_slug, window_start, window_end,
market_id, yes_asset_id, no_asset_id
```

`market_id` is the integer Polymarket market id (stringified for CSV);
`yes_asset_id` and `no_asset_id` are the two `clobTokenIds` entries
(index 0 = yes/up, index 1 = no/down per Polymarket's `outcomes` ordering).
Each rolling window is a fresh Polymarket market with a fresh ID and fresh
token pair, so a new row lands on every interval boundary.

The fetcher is idempotent: existing `window_start` rows are skipped on
re-runs, and the live loop checks the CSV before fetching.

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
│  │ Hyperliquid  │ ─►    .on::<OrderbookSnapshot, _>(|s| ...)     │  │
│  │   Client     │  │    .on::<OrderbookUpdate, _>(|u| ...)       │  │
│  └──────────────┘  │    .on::<UserEvent, _>(|e| ...)             │  │
│                    │                                             │  │
│  ┌──────────────┐  │  broadcast::Sender<StreamEvent>             │  │
│  │   Kalshi     │ ─►    OrderbookRaw(OrderbookUpdate)            │  │
│  │   Client     │  │    OrderbookSnapshot(OrderbookSnapshot)     │  │
│  └──────────────┘  │    User(UserEvent)                          │  │
│                    └────────────────┬────────────────────────────┘  │
└─────────────────────────────────────│────────────────────────────────┘
                                      │ StreamEngineBus
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
  → ClientBase.handle_message()
  → MsgParserTrait.parse_message()     → Vec<StreamMessage>
  → mpsc channel
  → StreamEngine loop
      ├─► HookRegistry.fire(&update)        (sync, before apply)
      ├─► broadcast OrderbookRaw(update)
      ├─► Orderbook.apply_update(update)    (Arc::make_mut copy-on-write)
      ├─► HookRegistry.fire(&snapshot)      (sync, after apply)
      └─► broadcast OrderbookSnapshot(snap) → ZmqServer PUB (every update)
```

---

## Rust Usage

### Construction pattern

Register hooks on the engine **before** handing it to `StreamSystem`:

```rust
use orderbook::{StreamEngine, StreamSystem, StreamSystemConfig};
use orderbook::connection::{ClientConfig, SystemControl};
use libs::protocol::{ExchangeName, OrderbookSnapshot};

let mut engine = StreamEngine::new(1024, 20);  // (broadcast_capacity, snapshot_depth)

engine.hooks_mut().on::<OrderbookSnapshot, _>(|snap: &OrderbookSnapshot| {
    println!("[{}:{}] bid={:?} ask={:?}", snap.exchange, snap.symbol, snap.best_bid, snap.best_ask);
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

### Broadcast subscribers (async, out-of-task)

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
// Register before engine.start() / StreamSystem::new()
engine.hooks_mut().on::<OrderbookSnapshot, _>(|s: &OrderbookSnapshot| { ... });
engine.hooks_mut().on::<OrderbookUpdate, _>(|u: &OrderbookUpdate| { ... });
engine.hooks_mut().on::<UserEvent, _>(|e: &UserEvent| { ... });

// Hooks fire synchronously inside the engine task, after broadcast, in registration order.
// Keep handlers cheap and non-blocking — a slow hook slows the engine loop.
```

### StreamEvent variants

| Variant | When | Payload |
|---|---|---|
| `OrderbookRaw(OrderbookUpdate)` | Before update is applied | Raw wire data |
| `OrderbookSnapshot(OrderbookSnapshot)` | After update is applied | Full materialized book |
| `User(UserEvent)` | On private channel event | Fill, OrderUpdate, Balance, Position |

### OrderbookSnapshot fields

```rust
pub exchange:  ExchangeName
pub symbol:    String
pub sequence:  u64
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
// Futures (depth only)
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .build()   // -> String

// Spot (depth + raw trades)
BinanceSubMsgBuilder::new()
    .with_orderbook_channel(&["btcusdt", "ethusdt"])
    .with_trade_channel(&["btcusdt", "ethusdt"])
    .build()   // -> String — pair with ExchangeName::BinanceSpot
```

### OKX

```rust
OkxSubMsgBuilder::new()
    .with_orderbook_channel_multi(vec!["BTC-USDT", "ETH-USDT-SWAP"], "SPOT")
    .build()   // -> String
```

### Polymarket

```rust
PolymarketSubMsgBuilder::new()
    .with_asset("71321045679252212594626385532706912750332728571942532289631379312455583992563")
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

All payloads are **msgpack** (`rmp-serde`). All sockets are IPC under `/tmp/trading/` (constants in `libs::constants`).

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Socket                   Pattern    Direction   Payload encoding        │
├─────────────────────────────────────────────────────────────────────────┤
│  MARKET_DATA_SOCKET       PUB/SUB    server→sub  msgpack                 │
│  ipc:///tmp/trading/market_data.sock                                    │
│                                                                         │
│  router_endpoint          ROUTER     client→srv  JSON (control only)    │
│  (config: tcp://*:5556)   /DEALER    server→cli  JSON (ack)             │
│                                                                         │
│  WS_STATUS_SOCKET         PUB/SUB    server→sub  msgpack                │
│  ipc:///tmp/trading/ws_status.sock                                      │
│                                                                         │
│  EXECUTOR_ORDER_SOCKET    REQ/REP    engine→exe  msgpack                │
│  ipc:///tmp/trading/executor_orders.sock                                │
│                                                                         │
│  CONTROL_SOCKET           PUB/SUB    backend→all msgpack                │
│  ipc:///tmp/trading/control.sock                                        │
└─────────────────────────────────────────────────────────────────────────┘
```

---

### Market PUB — `MARKET_DATA_SOCKET`

Broadcasts orderbook frames on every engine update. Frame: `[topic_bytes, msgpack_payload]`.

**Topic:** `"{exchange}:{symbol}"` — e.g. `"binance:btcusdt"`, `"polymarket:<token_id>"`

Clients filter by topic prefix using ZMQ's built-in `SUBSCRIBE` option. They must also send a subscribe request to the ROUTER first so the server tracks them in its registry (see below).

**Payload types** — selected at subscribe time via `"type"` field:

| Type | msgpack fields |
|---|---|
| `snapshot` | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread`, `mid`, `wmid`, `bids`, `asks` |
| `bba` | `exchange`, `symbol`, `sequence`, `timestamp`, `best_bid`, `best_ask`, `spread` |

`best_bid` / `best_ask` are `[price, qty]` tuples or `null`. `bids` / `asks` are arrays of `[price, qty]`, best-first.

`exchange` is encoded as a single-key msgpack map, e.g. `{"binance": 0}` — the Python subscriber normalises this automatically (see example script).

---

### ROUTER — `router_endpoint`

**Control plane only.** Clients connect a DEALER socket to subscribe or unsubscribe. All frames are JSON. The ROUTER never sends market data — it only sends acks.

**Subscribe:**
```json
{"action": "subscribe", "exchange": "binance", "symbol": "btcusdt", "type": "snapshot"}
```

**Unsubscribe:**
```json
{"action": "unsubscribe", "exchange": "binance", "symbol": "btcusdt"}
```

| Field | Values |
|---|---|
| `action` | `subscribe` \| `unsubscribe` |
| `exchange` | `binance` `binance_spot` `okx` `polymarket` `hyperliquid` `kalshi` |
| `symbol` | Exchange-native format — `btcusdt`, `BTC-USDT`, `<token_id>`, `KXBTC15M-…` |
| `type` | `snapshot` \| `bba` — required on subscribe, ignored on unsubscribe |

**Ack** (always sent back to the requesting client):
```json
{"status": "ok", "exchange": "binance", "symbol": "btcusdt", "type": "snapshot"}
{"status": "error", "message": "parse error: ..."}
```

---

### User-event PUB — `WS_STATUS_SOCKET`

Broadcasts private account events. No subscription handshake — clients connect a SUB socket and set a topic filter. Frame: `[topic_bytes, msgpack_payload]`.

**Topic:** `"user.{exchange}.{kind}"`

| Kind | Topic example | When fired |
|---|---|---|
| `fill` | `user.polymarket.fill` | Trade executed |
| `order_update` | `user.kalshi.order_update` | Order placed / partially filled / cancelled |
| `balance` | `user.binance.balance` | Account balance changed |
| `position` | `user.hyperliquid.position` | Position updated |

**Payload** — msgpack of `UserEvent` (tagged union, `"type"` field discriminates):

```
Fill:        { type, exchange, client_oid, exchange_oid, symbol, side, px, qty, fee, ts_ns }
OrderUpdate: { type, exchange, client_oid, exchange_oid, symbol, side, px, qty, filled, status, ts_ns }
Balance:     { type, exchange, asset, free, locked, ts_ns }
Position:    { type, exchange, symbol, size, avg_px, ts_ns }
```

`side` is `"buy"` or `"sell"`. `status` is one of `"new"`, `"partially_filled"`, `"filled"`, `"canceled"`, `"rejected"`, `"expired"`.

---

### Executor REQ/REP — `EXECUTOR_ORDER_SOCKET`

**Order placement gateway.** The Python engine sends `OrderRequest` and receives `OrderResponse` synchronously. Frame: raw msgpack (no topic prefix — REQ/REP is point-to-point).

**Request** (`OrderRequest`):
```
{ req_id: u64, exchange: ExchangeName, action: OrderAction }
```

**`OrderAction` variants:**

| Action | Extra fields |
|---|---|
| `place` | `client_oid`, `symbol`, `side`, `kind` (`limit`\|`market`), `px`, `qty`, `tif` (`GTC`\|`IOC`\|`FOK`\|`GTD`) |
| `place_batch` | `orders: [PlaceOne, ...]` |
| `update` | `client_oid`, `exchange_oid`, `new_px?`, `new_qty?` |
| `cancel` | `exchange_oid` |
| `cancel_all` | — |
| `cancel_market` | `symbol` |
| `heartbeat` | — (dead-man-switch ping; executor cancels all orders if missed) |

**Response** (`OrderResponse`):
```
{ req_id: u64, result: Ok(OrderResult) | Err(OrderError) }
```

`OrderResult` variants: `ack`, `batch_ack`, `cancel_count`, `heartbeat_ok`.
`OrderError` variants: `risk_rejected`, `kill_switch`, `duplicate_client_oid`, `exchange_rejected`, `network`, `timeout`, `not_found`, `internal`.

---

### Control PUB — `CONTROL_SOCKET`

**Broadcast control messages** from `backend` or CLI to all services. Clients subscribe with no topic filter (receive all). Frame: `[topic_bytes, msgpack_payload]` where topic is the message type.

**`ControlMessage` variants** (msgpack tagged by `"type"` field):

| Type | Fields | Effect |
|---|---|---|
| `shutdown` | — | All services stop cleanly |
| `pause` | `service: String` | Named service pauses processing |
| `resume` | `service: String` | Named service resumes |
| `strategy_params` | `(JSON value)` | Engine reloads strategy parameters |
| `terminate_strategy` | — | Engine stops current strategy |

---

## Python Client

### Minimal — subscribe via DEALER then read PUB

```python
import zmq, msgpack

ctx = zmq.Context()

dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({"action": "subscribe", "exchange": "binance", "symbol": "BTCUSDT", "type": "snapshot"})
print("ack:", dealer.recv_json())

sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    topic, payload = sub.recv_multipart()
    snap = msgpack.unpackb(payload, raw=False)
    print(f"bid={snap['best_bid']}  ask={snap['best_ask']}  wmid={snap['wmid']:.4f}")
```

### Full subscriber script

```bash
python3 zmq_server/examples/orderbook_subscriber.py --exchange binance --symbol BTCUSDT
python3 zmq_server/examples/orderbook_subscriber.py --exchange okx --symbol ETH-USDT-SWAP --type bba
python3 zmq_server/examples/orderbook_subscriber.py --exchange hyperliquid --symbol BTC
```

> **Note:** `ExchangeName` is encoded as a single-key msgpack map, e.g. `{"Binance": 0}`.
> The subscriber script normalises this automatically.

---

## Redis Integration

When `redis.enabled: true`, `orderbook_server` writes market data to Redis on every engine tick:

| Key pattern | Type | Contents |
|---|---|---|
| `ob:{exchange}:{symbol}` | String | Latest full orderbook snapshot (msgpack) |
| `bba:{exchange}:{symbol}` | String | Latest best-bid-ask (msgpack) |
| `snapshots:{exchange}:{symbol}` | List | Recent snapshots, capped at `snapshot_cap` |
| `trades:{exchange}:{symbol}` | List | Recent last-trade events, capped at `trade_cap` |
| `position:{exchange}:{symbol}` | String | Latest position (msgpack, from user channel) |
| `balance:{exchange}:{asset}` | String | Latest balance (msgpack, from user channel) |
| `events:fills` | List | Recent fill events, capped at `event_list_cap` |
| `events:orders` | List | Recent order updates, capped at `event_list_cap` |

All values are msgpack-encoded. Lists use `LPUSH + LTRIM` so index 0 is always the most recent entry.

Start Redis via Docker Compose (included in repo):

```bash
docker compose up redis -d
```

---

## HTTP Backend

`backend` is a lightweight Axum HTTP server that reads the Redis keys above and exposes them as JSON.

```bash
cargo run --bin backend --release -- --config configs/example.yaml
```

### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | `{"ok": true}` |
| `GET` | `/api/orderbook/:exchange/:symbol` | Latest full snapshot |
| `GET` | `/api/bba/:exchange/:symbol` | Latest best-bid-ask |
| `GET` | `/api/snapshots/:exchange/:symbol?limit=N` | Recent N snapshots (default 20) |
| `GET` | `/api/trades/:exchange/:symbol?limit=N` | Recent N last-trade events (default 20) |

`:exchange` matches the lowercase exchange name (`binance`, `binance_spot`, `okx`, `polymarket`, `hyperliquid`, `kalshi`). `:symbol` is the exchange-native symbol (e.g. `btcusdt`, `BTC-USDT`, `<token_id>`).

Responses are JSON. Missing keys return `{"error": "..."}` with status `404`.

---

## Server Configuration

All options are set via the YAML config file. Only `--config`, `--log-level`, and `--log-json` are accepted as CLI flags.

| CLI flag | Env var | Default | Description |
|---|---|---|---|
| `--config` | `CONFIG_FILE` | `configs/server.yaml` | Path to YAML config |
| `--log-level` | `LOG_LEVEL` | `info` | Tracing filter string |
| `--log-json` | `LOG_JSON` | `false` | Emit JSON-formatted logs |

```bash
cargo run --bin orderbook_server --release -- --config configs/server.yaml
cargo run --bin orderbook_server --release -- --config configs/server.yaml --log-level debug
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

