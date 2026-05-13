# ZMQ Wire Protocol

All payloads are **msgpack** (`rmp-serde`). All IPC sockets live under `/tmp/trading/`. Socket constants in `libs::constants`; types in `libs/src/protocol/`.

---

## ZMQ in 60 seconds (for newcomers)

ZMQ is **not** a message broker — there is no central server like RabbitMQ. It is a library that gives your sockets superpowers: automatic reconnection, message framing, in-memory queueing, and built-in *patterns* that decide how peers talk to each other. A "ZMQ socket" looks like a BSD socket but transports complete *messages* (not byte streams), and you pick a **pattern** by socket type.

Each pattern is a contract about who can talk to whom:

| Pattern | Direction | Used in this project for |
|---|---|---|
| **PUB / SUB** | one-to-many, fire-and-forget | market data, user events, control broadcasts |
| **ROUTER / DEALER** | many-to-one, async request/reply | subscription handshake, executor orders |
| **REQ / REP** | strict synchronous request/reply | (alternative for executor; we use ROUTER/DEALER for parallelism) |

Two more things to know up front:

- **Transports** — `tcp://`, `ipc://` (Unix domain socket), `inproc://` (in-process). Same API, different speed. We use `ipc://` because every service runs on one box. IPC is faster than TCP because it skips the kernel network stack.
- **HWM (high-water mark)** — every socket has a per-peer in-memory queue. When the queue fills, the socket either **drops** messages (PUB) or **blocks** the sender (DEALER/PUSH). Default HWM is 1000 messages. This is the most common surprise for new ZMQ users.

---

## Socket map

| Constant | Path / endpoint | Pattern | Encoding |
|---|---|---|---|
| `MARKET_DATA_SOCKET` | `ipc:///tmp/trading/market_data.sock` (tcp `:5555`) | PUB → SUB | msgpack |
| `router_endpoint` | `tcp://*:5556` | ROUTER ↔ DEALER | **JSON** |
| `WS_STATUS_SOCKET` | `ipc:///tmp/trading/ws_status.sock` | PUB → SUB | msgpack |
| `EXECUTOR_ORDER_SOCKET` | `ipc:///tmp/trading/executor_orders.sock` (tcp `:5557`) | ROUTER ↔ DEALER (REQ/REP-style) | msgpack |
| `CONTROL_SOCKET` | `ipc:///tmp/trading/control.sock` | PUB → SUB | msgpack |
| `ENGINE_METRICS_SOCKET` | `ipc:///tmp/trading/engine_metrics.sock` | PUB → SUB | msgpack |

---

## Pattern 1 — PUB / SUB (broadcast)

```
                     ┌──── SUB (Python: chart)
PUB (orderbook_server) ──┼──── SUB (recorder)
                     └──── SUB (backend)
```

**The contract:** the PUB socket *broadcasts*; every SUB that has set a matching topic filter gets a copy. The PUB never knows who is listening and never blocks waiting for them. **If a SUB is slow or absent, messages destined for it are silently dropped** (after `SNDHWM` is reached). PUB/SUB is the right choice when stale data is useless — there is no point queueing 10s-old orderbook snapshots.

**Message framing.** Every message we send on a PUB socket is **two frames**:

```
[ topic_bytes ] [ msgpack_payload ]
```

The first frame is the topic string (e.g. `b"binance:btcusdt"`). The SUB socket filters on a **prefix match** of the first frame *before* the second frame is even delivered to userspace — so a SUB that subscribes to `b"binance:"` only ever sees Binance frames, even though OKX/Polymarket/Kalshi frames are flowing through the same socket.

**How to use it in Python:**

```python
import zmq, msgpack
ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")    # prefix filter
# Subscribe to *everything*: sub.setsockopt(zmq.SUBSCRIBE, b"")
# Multiple filters allowed — repeat setsockopt to subscribe to more prefixes.

while True:
    topic, payload = sub.recv_multipart()            # always two frames
    snap = msgpack.unpackb(payload, raw=False)
```

**Slow-joiner trap.** PUB sends messages the moment they are produced, even if no SUB has finished connecting. A SUB that connects *after* a message was sent will never see it — there is no replay. In practice: if you start your SUB program a millisecond after orderbook_server, you may miss the first few snapshots. The next snapshot arrives within ~100 ms so this rarely matters here, but be aware when writing tests.

**Capacity / HWM.** PUB defaults to `SNDHWM = 1000` per connected SUB. If a SUB lags, its queue fills, and *that SUB's* future messages are dropped — other SUBs are unaffected. Our orderbook traffic is ~10–100 msgs/sec per symbol; the default is plenty. Increase via `pub.setsockopt(zmq.SNDHWM, N)` if you start dropping.

### Used by

- **`MARKET_DATA_SOCKET`** — `orderbook_server` publishes every `OrderbookSnapshot` or `BbaPayload`. Topic: `"{exchange}:{symbol}"` (e.g. `binance:btcusdt`, `polymarket:<token_id>`). Subscribers also send a JSON handshake on the ROUTER (see Pattern 2) so the server knows which payload type (`snapshot` vs `bba`) to publish to that client — the PUB itself has no knowledge of clients.
- **`WS_STATUS_SOCKET`** — user-channel events (`UserEvent`). Topic: `"user.{exchange}.{kind}"` where `kind ∈ {fill, order_update, balance, position}`. No handshake — just set the SUB filter.
- **`CONTROL_SOCKET`** — broadcasts `ControlMessage` (`shutdown`, `pause`, `resume`, `strategy_params`, `terminate_strategy`) to every service. Subscribers usually connect with no filter (`b""`) to receive all.
- **`ENGINE_METRICS_SOCKET`** — internal metrics from the engine to the backend.

---

## Pattern 2 — ROUTER / DEALER (async request/reply)

```
DEALER (client) ─────► ROUTER (server)
DEALER (client) ─────►  routes by identity
DEALER (client) ─────►
```

**The contract:** ROUTER and DEALER are the "smart" versions of REP and REQ. They let many clients talk to one server *asynchronously* — a server can have multiple in-flight requests and reply to them out of order. This is the pattern you reach for whenever REQ/REP feels too rigid.

The key trick is that ROUTER **prepends an identity frame** to every incoming message, telling you which DEALER it came from. To reply, you put that identity frame back on the front and send. ZMQ routes the reply to the correct DEALER.

**Frame layout on the ROUTER side:**

```
Incoming:  [ client_identity ] [ "" ] [ payload_frame_1 ] [ payload_frame_2 ] ...
Outgoing:  [ client_identity ] [ "" ] [ reply_frame ]
```

The empty delimiter frame is a leftover from REQ-compatibility — DEALER doesn't strictly need it, but most libraries (including ours) include it for symmetry with REQ/REP.

**Differences vs REQ/REP:**

| | REQ / REP | ROUTER / DEALER |
|---|---|---|
| Order | strict send→recv→send→recv | any order, any number in flight |
| State | breaks if you `send` twice in a row | totally fine |
| Identity | hidden | exposed (ROUTER side) |
| Use | scripts, simple cases | servers, real protocols |

**Capacity / HWM.** ROUTER defaults to `SNDHWM = RCVHWM = 1000`. When ROUTER's send queue to one DEALER is full, **new sends to that DEALER are dropped** (this is a special-case of ROUTER, unlike DEALER which would block). DEALER blocks when its queue is full. For our two ROUTER uses, traffic is low (one handshake per subscribe; one frame per order) so HWM is non-issue.

### Used by — control-plane ROUTER on `router_endpoint`

This socket handles **subscription handshakes** for the market PUB above. All frames are JSON (not msgpack — this is human-readable control traffic).

A client wanting to receive `snapshot` payloads must do the handshake **before** setting its SUB filter — orderbook_server tracks each client in a registry and uses that registry to decide what to publish.

```python
dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({"action":"subscribe","exchange":"binance","symbol":"btcusdt","type":"snapshot"})
ack = dealer.recv_json()    # {"status":"ok", ...} or {"status":"error","message":"..."}
```

Field reference (full schema in `velociraptor-zmq-protocol`):

- `action`: `"subscribe"` | `"unsubscribe"`
- `exchange`: lowercase enum name
- `symbol`: exchange-native (`btcusdt`, `BTC-USDT`, `<token_id>`, `KXBTC15M-…`)
- `type`: `"snapshot"` | `"bba"` (required on subscribe, ignored on unsubscribe)

The ROUTER *never* sends market data — only acks.

### Used by — executor ROUTER on `EXECUTOR_ORDER_SOCKET`

Same pattern, but the wire format is msgpack and the payload is `OrderRequest` / `OrderResponse`. The Python trading engine connects a DEALER (or `zmq.REQ` for simplicity) and gets one reply per request. Identity routing means the executor can handle multiple engines concurrently — one engine's slow order doesn't queue behind another's.

```python
req = ctx.socket(zmq.REQ)                              # REQ also works against a ROUTER
req.connect("ipc:///tmp/trading/executor_orders.sock")
req.send(msgpack.packb({
    "req_id": 1, "exchange": "polymarket", "action": "place",
    "client_oid": "my-order-001", "symbol": "<token_id>",
    "side": "buy", "kind": "limit", "px": 0.55, "qty": 10.0, "tif": "GTC",
}, use_bin_type=True))
response = msgpack.unpackb(req.recv(), raw=False)
```

`OrderAction` variants: `place`, `place_batch`, `update`, `cancel`, `cancel_all`, `cancel_market`, `heartbeat`. `heartbeat` is the **dead-man switch ping** — if the executor goes too long without one, it auto-cancels every open order. Full action schemas and `OrderError` variants live in `velociraptor-zmq-protocol`; REST mapping per exchange and risk gates live in `velociraptor-executor`.

---

## Pattern 3 — Why we don't use plain REQ / REP here

REQ/REP enforces a strict send-recv-send-recv lockstep. If either side gets out of sync (e.g. you call `send` twice without a `recv`), the socket goes into an unrecoverable error state. That's fine for a quick script but bad for a long-running engine and a many-client server. ROUTER/DEALER gives the same logical request/reply with no state machine to violate, plus the option to be fully async on either side.

The Python `zmq.REQ` socket is still safe to use **against** our ROUTER — REQ wraps the same identity-frame protocol under the hood. We just don't expose a server-side REP socket.

---

## Topics, filtering, and the prefix-match rule

Every PUB frame we send has a **topic frame** first. SUB filters are *prefix matches against the first frame*:

| Subscribe filter | Matches |
|---|---|
| `b""` | everything |
| `b"binance:"` | every Binance frame |
| `b"binance:BTCUSDT"` | exactly BTCUSDT on Binance |
| `b"user."` | every user event (any exchange, any kind) |
| `b"user.polymarket.fill"` | only Polymarket fills |

The filter is applied **in the SUB's userspace by default** — meaning a filtered-out message has still crossed the wire from PUB to SUB before being dropped. ZMQ also supports kernel-side filtering on PUB with `XPUB`/`XSUB` and `ZMQ_XPUB_VERBOSE`, but we don't use that — bandwidth on `ipc://` is essentially free.

A SUB can subscribe to multiple prefixes by calling `setsockopt(zmq.SUBSCRIBE, ...)` repeatedly. To unsubscribe from a prefix: `setsockopt(zmq.UNSUBSCRIBE, prefix)`.

---

## Endianness, threading, and other footnotes

- **Threading.** A `zmq.Context` is thread-safe. A `zmq.Socket` is **not** — never share a socket between threads/tasks without an explicit lock. In tokio, give each task its own socket (or use the `tokio-zmq` adapter).
- **Bind vs connect.** Servers `bind`; clients `connect`. The side that binds must come up first or use `transport-specific` reconnection (ZMQ handles reconnect automatically once both sides exist).
- **Linger.** On `close`/`term`, ZMQ defaults to a short linger so unsent messages can flush. Long-running services should set `LINGER=0` explicitly to avoid hangs on shutdown.
- **No backpressure across PUB/SUB.** A slow SUB cannot slow down the engine — it just starts dropping frames. If you need every snapshot, write a recorder that consumes from the *broadcast bus inside the engine* (Rust API: `engine.subscribe_event()`), not from the PUB socket.

---

## Sockets at a glance

- **Market PUB** — topic `"{exchange}:{symbol}"`, payload `OrderbookSnapshot` (full book) or `BbaPayload` (best bid / ask only). Type is selected per-client via the ROUTER handshake.
- **ROUTER (subs)** — JSON `{action, exchange, symbol, type}` → JSON `{status: "ok"|"error", ...}`. Control plane only.
- **User PUB** — topic `"user.{exchange}.{kind}"`, payload `UserEvent` tagged by `"type"` field (`fill` / `order_update` / `balance` / `position`).
- **Executor ROUTER** — msgpack `OrderRequest{req_id, exchange, action}` → `OrderResponse{req_id, result}`. Result is `Ok(OrderResult)` (`ack` / `batch_ack` / `cancel_count` / `heartbeat_ok`) or `Err(OrderError)` (`risk_rejected` / `kill_switch` / `duplicate_client_oid` / `exchange_rejected` / `network` / `timeout` / `not_found` / `internal`).
- **Control PUB** — `ControlMessage` tagged by `"type"`: `shutdown`, `pause`, `resume`, `strategy_params`, `terminate_strategy`.
- **Engine metrics PUB** — internal-only stats stream consumed by `backend`.

---

## Deep reference

Complete payload schemas, every `OrderAction` field, `OrderError` variants, `ExchangeName` enum encoding (`rmp-serde` quirk: enum variants serialize as single-key maps like `{"binance": 0}`), Python pydantic mirrors, and the dead-man-switch flow are documented in the project skill **`velociraptor-zmq-protocol`** at `.claude/skills/velociraptor-zmq-protocol/SKILL.md`.

Related skills:

- `velociraptor-python-clients` — pyzmq + msgpack recipes for every socket above
- `velociraptor-executor` — executor crate internals (REST mapping, risk gates, audit stream)

Recommended further reading on ZMQ itself:

- The ZMQ Guide, chapters 1–3 — https://zguide.zeromq.org (covers PUB/SUB and ROUTER/DEALER with worked examples)
- `man zmq_socket(3)` — terse but complete pattern reference
