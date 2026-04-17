# zmq_server

ZMQ transport layer for velociraptor. Binds three sockets and forwards events
from any `EngineEventSource` into them:

| Socket | Pattern | Purpose | Topic format |
|--------|---------|---------|--------------|
| market PUB | PUB | throttled orderbook snapshots | `{exchange}:{symbol}` |
| control ROUTER | ROUTER | subscribe / unsubscribe / add_channel | — (JSON req/reply) |
| user PUB | PUB | private user events (fills, orders, balances, positions) | `user.{exchange}.{fill\|order_update\|balance\|position}` |

Market-data and user-event payloads are **msgpack**. Control-channel payloads
are **JSON**.

The crate also hosts `TradingEngine` / `TradingSystem` — the demuxing engine
that produces the event stream the server consumes. See `src/trading/` for
details.

---

## Module layout

```
src/
├── server.rs        orchestrator — binds sockets, runs the dispatch loop
├── frame/           pure payload encoding (no I/O)
│   ├── market.rs    snapshot / BBA msgpack encoding + topic
│   └── user.rs      user-event topic + msgpack encoding
├── socket/          thin tmq wrappers
│   ├── pub_socket.rs
│   └── router.rs
├── control/         subscription registry + ROUTER handler
│   ├── registry.rs  Registry, apply_request, dispatch
│   └── handler.rs   parses requests, returns reply frames
├── trading/         TradingEngine, TradingSystem, EngineEventSource
├── protocol.rs      wire DTOs (Ack, SubscriptionRequest, BbaPayload)
└── types.rs         shared types (Action, SubscriptionKey, …)
```

---

## Quick start

### 1. Run the example server

```bash
cargo run --example zmq_publisher
```

Binds:
- market PUB  — `tcp://*:5555`
- ROUTER      — `tcp://*:5556`
- user  PUB   — `tcp://*:5557`

Streams Binance `btcusdt` / `ethusdt` and one Polymarket asset. Ctrl-C to stop.

### 2. Subscribe from Python

Install once: `pip install pyzmq msgpack`.

```bash
# Snapshot, throttled to 500ms
python3 zmq_server/examples/zmq_subscriber.py \
    --exchange binance --symbol btcusdt --type snapshot --interval 500

# Best bid/ask only, 100ms
python3 zmq_server/examples/zmq_subscriber.py \
    --exchange binance --symbol ethusdt --type bba --interval 100

# Ask the server to start a new channel at runtime
python3 zmq_server/examples/zmq_subscriber.py \
    --add-channel --exchange binance --symbol solusdt
```

> The bundled `zmq_subscriber.py` was written when snapshots were JSON.
> Payloads are now msgpack — swap `json.loads(payload)` for
> `msgpack.unpackb(payload, raw=False)`.

### 3. Production binary

The shipping binary is `zmq_server::bin::orderbook_server`:

```bash
cargo run --bin orderbook_server -- --config configs/server.yaml
```

Reads `configs/server.yaml` (see `configs/example.yaml` for the full schema)
and wires Binance / OKX / Hyperliquid per the config. CLI flags
(`--binance`, `--depth`, …) override the yaml; env vars override both.

---

## Using the library from your own binary

```rust
use libs::constants::WS_STATUS_SOCKET;
use libs::protocol::ExchangeName;
use orderbook::connection::{ConnectionConfig, SystemControl};
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use std::sync::Arc;
use zmq_server::{TradingSystem, TradingSystemConfig, ZmqServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Configure the engine: which exchanges, which symbols, snapshot depth.
    let mut cfg = TradingSystemConfig::new();
    cfg.set_snapshot_depth(10);
    cfg.with_exchange(
        ConnectionConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&["btcusdt"])
                .build(),
        ),
    );
    cfg.validate()?;

    // 2. Spawn the engine — it owns the WS connections and the event bus.
    let ctrl = SystemControl::new();
    let mut system = TradingSystem::new(cfg, ctrl.clone())?;

    // 3. Spawn the ZMQ server. It subscribes to the engine bus via the
    //    EngineEventSource trait — no direct coupling to the engine.
    let zmq_handle = ZmqServer::new(
        "tcp://*:5555",           // market PUB
        "tcp://*:5556",           // control ROUTER
        WS_STATUS_SOCKET,         // user PUB (ipc by default)
        system.channel_request_sender(),
    )
    .start(Arc::new(system.engine_bus()));
    system.attach_handle(zmq_handle);

    // 4. Run until Ctrl-C.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => ctrl.shutdown(),
        r = system.run() => r?,
    }
    Ok(())
}
```

`ZmqServer::start` returns a `JoinHandle<()>` — keep it alive (here via
`system.attach_handle`) or the transport task will be dropped.

---

## Control protocol (ROUTER)

Connect a DEALER socket; send JSON, receive JSON ack.

```jsonc
// Subscribe — server starts publishing on the market PUB
{ "action": "subscribe",   "exchange": "binance", "symbol": "btcusdt",
  "type": "snapshot", "interval": 500 }

// Unsubscribe
{ "action": "unsubscribe", "exchange": "binance", "symbol": "btcusdt" }

// Ask the server to start streaming a new channel it wasn't configured for
{ "action": "add_channel", "exchange": "binance", "symbol": "solusdt" }
```

Ack shape:
```jsonc
{ "status": "ok",  "exchange": "...", "symbol": "...", "type": "snapshot",
  "interval": 500 }
{ "status": "error", "message": "parse error: ..." }
```

`type` is `snapshot` (full depth) or `bba` (best bid/ask only). `interval`
is the per-client throttle in ms.

---

## Market-data payloads (msgpack)

### Snapshot — `type: "snapshot"`

```python
{
    "exchange":  "binance",
    "symbol":    "btcusdt",
    "sequence":  12345,
    "timestamp": "2026-04-17T08:15:30.123Z",
    "best_bid":  [62001.2, 0.5],     # [price, qty] | None
    "best_ask":  [62001.5, 0.3],
    "spread":    0.3,                # None if either side empty
    "mid":       62001.35,
    "wmid":      62001.4,             # volume-weighted mid
    "bids":      [[62001.2, 0.5], …], # top N, best first
    "asks":      [[62001.5, 0.3], …],
}
```

### BBA — `type: "bba"`

Same as above but only `exchange`, `symbol`, `sequence`, `timestamp`,
`best_bid`, `best_ask`, `spread`.

---

## User events (msgpack)

Published on the user PUB socket (default `WS_STATUS_SOCKET`
= `ipc:///tmp/trading/ws_status.sock`). Topic is
`user.{exchange}.{fill|order_update|balance|position}`. Payloads mirror
`libs::protocol::UserEvent` — see `libs/src/protocol/events.rs` for the
field-level schema and `docs/protocol.md` for the cross-language spec.

No subscription request needed — it's a plain PUB/SUB bus. Subscribe to the
topic prefix you care about:

```python
sub.setsockopt(zmq.SUBSCRIBE, b"user.polymarket.")   # everything for one exchange
sub.setsockopt(zmq.SUBSCRIBE, b"user.")              # everything
```

---

## Plugging in a non-`TradingSystem` source

`ZmqServer` only requires an `Arc<dyn EngineEventSource>`. Implement the
trait on your own type if you need to feed the server from elsewhere:

```rust
use tokio::sync::broadcast;
use zmq_server::{EngineEvent, EngineEventSource};

struct MyBus(broadcast::Sender<EngineEvent>);

impl EngineEventSource for MyBus {
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.0.subscribe()
    }
}
```

---

## Troubleshooting

- **"Address already in use"** — another process is bound to the same
  endpoint. ZMQ doesn't do SO_REUSEADDR; kill the other process or pick a
  different port / IPC path.
- **`ipc://` path not found** — on Linux/macOS the parent directory must
  exist. Defaults live under `/tmp/trading/`; create it once per boot.
- **Subscribers receive nothing but the server is running** — make sure
  you sent the subscribe request on the ROUTER *and* connected a SUB socket
  to the PUB endpoint, and that the SUB filter matches the topic
  (`{exchange}:{symbol}`, lowercase exchange name — see
  `ExchangeName::to_str`).
- **Snapshots decode as gibberish** — you're parsing msgpack as JSON. Use
  `msgpack.unpackb(payload, raw=False)` (Python) / `rmp_serde::from_slice`
  (Rust).
