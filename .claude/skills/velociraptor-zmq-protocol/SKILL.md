---
name: velociraptor-zmq-protocol
description: Complete ZMQ wire protocol for velociraptor — socket addresses, topic formats, msgpack payload schemas, OrderRequest/OrderResponse, OrderAction variants, ControlMessage, ExchangeName enum encoding. Use when implementing or debugging any ZMQ client or server.
---

# Velociraptor — ZMQ Wire Protocol

All payloads are **msgpack** (`rmp-serde` on Rust, `msgpack` on Python). All IPC sockets live under `/tmp/trading/`; trust boundary is the machine. No CURVE auth. Socket path constants live in `libs::constants`. Type defs live in `libs/src/protocol/`.

## Socket map

| Constant | Path / endpoint | Pattern | Direction | Encoding |
|---|---|---|---|---|
| `MARKET_DATA_SOCKET` | `ipc:///tmp/trading/market_data.sock` (also `tcp://*:5555` in compose) | PUB/SUB | server → sub | msgpack |
| `router_endpoint` | `tcp://*:5556` | ROUTER/DEALER | client ↔ server | **JSON** |
| `WS_STATUS_SOCKET` | `ipc:///tmp/trading/ws_status.sock` | PUB/SUB | server → sub | msgpack |
| `EXECUTOR_ORDER_SOCKET` | `ipc:///tmp/trading/executor_orders.sock` (also `tcp://*:5557`) | REQ/REP | engine → executor | msgpack |
| `CONTROL_SOCKET` | `ipc:///tmp/trading/control.sock` | PUB/SUB | backend → all | msgpack |
| `ENGINE_METRICS_SOCKET` | `ipc:///tmp/trading/engine_metrics.sock` | PUB/SUB | engine → backend | msgpack |

## ExchangeName encoding

`rmp-serde` serializes Rust enum variants as a single-key msgpack map — `{"binance": 0}`. Python clients must unwrap: `exchange = next(iter(raw["exchange"]))`. The `bba` payload uses a plain string instead.

---

## Market PUB — `MARKET_DATA_SOCKET`

Frame: `[topic_bytes, msgpack_payload]`. Topic is the STABLE identifier — so subscribers don't have to resubscribe on rollover:

| Exchange class | Topic format | Example |
|---|---|---|
| Static (`binance`/`binance_spot`/`okx`/`hyperliquid`) | `{exchange}:{symbol}` | `binance:btcusdt` |
| Polymarket rolling | `polymarket:{base_slug}` | `polymarket:btc-updown-15m` |
| Kalshi rolling | `kalshi:{series}` | `kalshi:KXBTC15M` |
| Last-trade variant | `{topic}:last_trade` | `polymarket:btc-updown-15m:last_trade` |

For rolling markets, only the UP side is published (Polymarket DOWN tokens are a mirror image of UP; Kalshi has no up/down split). The current window's per-window identifier is carried inside the payload as `full_slug` — see below — so a subscriber on the stable topic detects rollover from the payload, no resubscribe needed.

Clients set a ZMQ `SUBSCRIBE` filter AND send a subscribe request on ROUTER so the server records the payload type. Two payload types, selected at subscribe via the `"type"` field:

**`snapshot` (OrderbookSnapshot):**
```
exchange   : ExchangeName    # single-key map e.g. {"binance": 0}
symbol     : str             # exchange-native unit:
                             #   static — the symbol
                             #   Polymarket — asset_id of the UP token
                             #   Kalshi — the current window's ticker
full_slug  : str | null      # rolling markets only — current window's
                             # full_slug (Polymarket) or ticker (Kalshi).
                             # null/absent for static exchanges.
                             # Encoded as `#[serde(default)] Option<String>`
                             # so old captures decode under the new schema.
sequence   : u64
timestamp  : DateTime<Utc>
best_bid   : [price, qty] | null
best_ask   : [price, qty] | null
spread     : f64 | null
mid        : f64 | null
wmid       : f64             # quantity-weighted mid
bids       : [[price, qty]]  # best first
asks       : [[price, qty]]
```

**`bba` (BbaPayload):**
```
exchange   : str             # plain string
symbol     : str
full_slug  : str | null      # mirrors OrderbookSnapshot.full_slug
sequence   : u64
timestamp  : DateTime<Utc>
best_bid   : [price, qty] | null
best_ask   : [price, qty] | null
spread     : f64 | null
```

**`last_trade` (LastTradePrice):** same `full_slug` field; published on the `:last_trade` topic suffix above.

### Internal event variants

The Rust-side `StreamEvent` enum carries the dispatch info needed to pick the static topic. Don't remove the rolling variants — server dispatch (`zmq_server/src/server.rs`) needs `base_slug` from the variant to pick `RollingSnapshotTopic` / `RollingLastTradeTopic` over the per-symbol topic:

```rust
StreamEvent::OrderbookSnapshot(OrderbookSnapshot)       // static exchanges
StreamEvent::RollingSnapshot { base_slug, snap }        // rolling — publishes on `{exchange}:{base_slug}`
StreamEvent::LastTradePrice(LastTradePrice)             // static
StreamEvent::RollingLastTradePrice { base_slug, trade } // rolling — publishes on `{exchange}:{base_slug}:last_trade`
```

The per-window forward hook in `spawn_polymarket_window` / `spawn_kalshi_window` is what stamps `full_slug` into the payload and publishes the rolling variant via `engine_bus.publish(...)`. Static exchanges go through the engine's normal hook path.

---

## ROUTER — `router_endpoint` (control plane, JSON)

Clients connect a DEALER. The ROUTER never sends market data — only acks.

**Subscribe / Unsubscribe:**
```json
{"action":"subscribe",  "exchange":"binance","symbol":"btcusdt","type":"snapshot"}
{"action":"unsubscribe","exchange":"binance","symbol":"btcusdt"}
```

| Field | Values |
|---|---|
| `action` | `subscribe` \| `unsubscribe` |
| `exchange` | `binance` \| `binance_spot` \| `okx` \| `polymarket` \| `hyperliquid` \| `kalshi` |
| `symbol` | Exchange-native (`btcusdt`, `BTC-USDT`, `<token_id>`, `KXBTC15M-…`) |
| `type` | `snapshot` \| `bba` (required on subscribe) |

**Ack:**
```json
{"status":"ok","exchange":"binance","symbol":"btcusdt","type":"snapshot"}
{"status":"error","message":"parse error: ..."}
```

---

## User-event PUB — `WS_STATUS_SOCKET`

No handshake. Frame: `[topic_bytes, msgpack_payload]`. Topic: `"user.{exchange}.{kind}"` — `fill`, `order_update`, `balance`, `position`. Subscribe to `b"user."` for all.

**`UserEvent`** — tagged union, discriminated by `"type"`:

```
Fill:        type, exchange, client_oid, exchange_oid, symbol, side, px, qty, fee, ts_ns
OrderUpdate: type, exchange, client_oid, exchange_oid, symbol, side, px, qty, filled, status, ts_ns
Balance:     type, exchange, asset, free, locked, ts_ns
Position:    type, exchange, symbol, size, avg_px, ts_ns
```

`side` ∈ `"buy"|"sell"`. `status` ∈ `"new"|"partially_filled"|"filled"|"canceled"|"rejected"|"expired"`.

---

## Executor REQ/REP — `EXECUTOR_ORDER_SOCKET`

Point-to-point, no topic frame. Engine sends `OrderRequest`, blocks on `OrderResponse`.

**`OrderRequest`:**
```
req_id   : u64
exchange : ExchangeName
action   : OrderAction   # tagged by "action" field
```

**`OrderAction` variants:**

| `action` | Extra fields |
|---|---|
| `place` | `client_oid`, `symbol`, `side`, `kind`(`limit`\|`market`), `px`, `qty`, `tif`(`GTC`\|`IOC`\|`FOK`\|`GTD`) |
| `place_batch` | `orders: [PlaceOne]` |
| `update` | `client_oid`, `exchange_oid`, `new_px?`, `new_qty?` |
| `cancel` | `exchange_oid` |
| `cancel_all` | — |
| `cancel_market` | `symbol` |
| `heartbeat` | — (dead-man-switch ping) |

`PlaceOne = { client_oid, symbol, side, kind, px, qty, tif }`

**`OrderResponse`:**
```
req_id : u64
result : Ok(OrderResult) | Err(OrderError)
```

`OrderResult` (tagged by `"result"`): `ack` (`client_oid`,`exchange_oid`,`status`,`ts_ns`), `batch_ack` (`results:[...]`), `cancel_count` (`count`), `heartbeat_ok` (`next_due_ms`).

`OrderError` (tagged by `"kind"`): `risk_rejected` · `kill_switch` · `duplicate_client_oid` · `exchange_rejected` · `network` · `timeout` · `not_found` · `internal`.

**Heartbeat / dead-man:** engine sends `heartbeat` at cadence shorter than `next_due_ms` (e.g. every 5s for a 15s window). If executor misses `2 × interval` → calls `cancel_all` on every exchange + flips `risk:kill_switch="true"` in Redis. Top-level safety rail above all risk checks.

Symbol conventions per exchange:
- **Kalshi:** `"{ticker}.YES"` or `"{ticker}.NO"` — suffix selects yes/no.
- **Polymarket:** decimal token-id string.

---

## Control PUB — `CONTROL_SOCKET`

Subscribers connect with no filter (receive all). Frame: `[topic_bytes, msgpack_payload]` where topic is the type.

**`ControlMessage`** (tagged by `"type"`):

| `type` | Fields | Effect |
|---|---|---|
| `shutdown` | — | All services stop cleanly |
| `pause` | `service: str` | Named service pauses |
| `resume` | `service: str` | Named service resumes |
| `strategy_params` | JSON | Engine reloads strategy params |
| `terminate_strategy` | — | Engine stops current strategy |

---

## `libs/src/protocol/` module layout

| Module | Types |
|---|---|
| `mod.rs` | `ExchangeName`, `OrderbookSnapshot`, `PriceLevelTuple` |
| `events.rs` | `UserEvent`, `EventKind` |
| `orders.rs` | `OrderRequest`, `OrderResponse`, `OrderAction`, `OrderResult`, `OrderError`, `OrderAck`, `HeartbeatAck`, `PlaceOne`, `Side`, `OrderKind`, `Tif`, `OrderStatus` |
| `control.rs` | `ControlMessage` |

Keep Python pydantic models in lockstep with these Rust enums — any change must be reflected before deployment.
