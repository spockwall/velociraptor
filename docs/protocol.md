# ZMQ Wire Protocol

All ZMQ payloads are **msgpack** (`rmp-serde` on Rust, `msgpack` on Python).
All sockets are IPC-only under `/tmp/trading/` — the trust boundary is the
machine. No CURVE authentication.

Socket path constants live in `libs::constants`. Type definitions live in
`libs/src/protocol/` (see module layout below).

---

## Socket Map

| Constant | Path | Pattern | Direction | Encoding |
|---|---|---|---|---|
| `MARKET_DATA_SOCKET` | `ipc:///tmp/trading/market_data.sock` | PUB/SUB | orderbook-server → subscribers | msgpack |
| *(config field)* | `router_endpoint` (default `tcp://*:5556`) | ROUTER/DEALER | client ↔ orderbook-server | JSON |
| `WS_STATUS_SOCKET` | `ipc:///tmp/trading/ws_status.sock` | PUB/SUB | orderbook-server → subscribers | msgpack |
| `EXECUTOR_ORDER_SOCKET` | `ipc:///tmp/trading/executor_orders.sock` | REQ/REP | engine → executor | msgpack |
| `CONTROL_SOCKET` | `ipc:///tmp/trading/control.sock` | PUB/SUB | backend/CLI → all services | msgpack |
| `ENGINE_METRICS_SOCKET` | `ipc:///tmp/trading/engine_metrics.sock` | PUB/SUB | engine → backend | msgpack |

---

## Market PUB — `MARKET_DATA_SOCKET`

Frame: `[topic_bytes, msgpack_payload]`

**Topic:** `"{exchange}:{symbol}"` — e.g. `"binance:btcusdt"`, `"polymarket:<token_id>"`

Clients set a ZMQ topic filter (`SUBSCRIBE` option) and must also send a
subscribe request to the ROUTER so the server records the desired payload
type in its registry.

### Payload types

Selected at subscribe time via `"type"` field on the ROUTER request.

**`snapshot`** — full materialized orderbook (`OrderbookSnapshot`):

```
exchange   : ExchangeName   # encoded as single-key map e.g. {"binance": 0}
symbol     : str
sequence   : u64
timestamp  : DateTime<Utc>
best_bid   : [price, qty] | null
best_ask   : [price, qty] | null
spread     : f64 | null
mid        : f64 | null
wmid       : f64            # quantity-weighted mid
bids       : [[price, qty]] # best first
asks       : [[price, qty]] # best first
```

**`bba`** — best-bid-ask only (`BbaPayload`):

```
exchange   : str            # plain string, not the enum map
symbol     : str
sequence   : u64
timestamp  : DateTime<Utc>
best_bid   : [price, qty] | null
best_ask   : [price, qty] | null
spread     : f64 | null
```

> **`ExchangeName` encoding note:** `rmp-serde` serializes Rust enum variants
> as a single-key map `{"variantname": 0}`. The Python subscriber must unwrap
> this — e.g. `exchange = list(raw["exchange"].keys())[0]`.

---

## ROUTER — `router_endpoint`

**Control plane only.** Clients connect a DEALER socket. All frames are JSON.
The ROUTER never sends market data — only acks.

### Subscribe

```json
{"action": "subscribe", "exchange": "binance", "symbol": "btcusdt", "type": "snapshot"}
```

### Unsubscribe

```json
{"action": "unsubscribe", "exchange": "binance", "symbol": "btcusdt"}
```

| Field | Values |
|---|---|
| `action` | `subscribe` \| `unsubscribe` |
| `exchange` | `binance` `okx` `polymarket` `hyperliquid` `kalshi` |
| `symbol` | Exchange-native — `btcusdt`, `BTC-USDT`, `<token_id>`, `KXBTC15M-…` |
| `type` | `snapshot` \| `bba` — required on subscribe |

### Ack

```json
{"status": "ok", "exchange": "binance", "symbol": "btcusdt", "type": "snapshot"}
{"status": "error", "message": "parse error: missing field 'type'"}
```

---

## User-event PUB — `WS_STATUS_SOCKET`

Frame: `[topic_bytes, msgpack_payload]`

No subscription handshake — clients connect a SUB socket and set a topic
filter directly.

**Topic:** `"user.{exchange}.{kind}"`

| Kind | Topic example | Fired when |
|---|---|---|
| `fill` | `user.polymarket.fill` | Trade executed |
| `order_update` | `user.kalshi.order_update` | Order placed / partially filled / cancelled |
| `balance` | `user.binance.balance` | Account balance changed |
| `position` | `user.hyperliquid.position` | Position updated |

### Payload — `UserEvent` (tagged by `"type"` field)

```
Fill:
  type         : "fill"
  exchange     : str
  client_oid   : str
  exchange_oid : str
  symbol       : str
  side         : "buy" | "sell"
  px           : f64
  qty          : f64
  fee          : f64
  ts_ns        : i64

OrderUpdate:
  type         : "order_update"
  exchange     : str
  client_oid   : str
  exchange_oid : str
  symbol       : str
  side         : "buy" | "sell"
  px           : f64
  qty          : f64
  filled       : f64
  status       : "new" | "partially_filled" | "filled" | "canceled" | "rejected" | "expired"
  ts_ns        : i64

Balance:
  type         : "balance"
  exchange     : str
  asset        : str
  free         : f64
  locked       : f64
  ts_ns        : i64

Position:
  type         : "position"
  exchange     : str
  symbol       : str
  size         : f64
  avg_px       : f64
  ts_ns        : i64
```

---

## Executor REQ/REP — `EXECUTOR_ORDER_SOCKET`

Point-to-point — no topic frame. Frame: `[msgpack_payload]`.

The engine sends `OrderRequest` and blocks until it receives `OrderResponse`.

### `OrderRequest`

```
req_id   : u64
exchange : ExchangeName
action   : OrderAction   # tagged by "action" field
```

### `OrderAction` variants

| `action` | Extra fields |
|---|---|
| `place` | `client_oid`, `symbol`, `side`, `kind` (`limit`\|`market`), `px`, `qty`, `tif` (`GTC`\|`IOC`\|`FOK`\|`GTD`) |
| `place_batch` | `orders: [PlaceOne]` |
| `update` | `client_oid`, `exchange_oid`, `new_px?`, `new_qty?` |
| `cancel` | `exchange_oid` |
| `cancel_all` | — |
| `cancel_market` | `symbol` |
| `heartbeat` | — (dead-man-switch ping) |

`PlaceOne = { client_oid, symbol, side, kind, px, qty, tif }`

### `OrderResponse`

```
req_id : u64
result : Ok(OrderResult) | Err(OrderError)
```

### `OrderResult` variants (tagged by `"result"` field)

| `result` | Fields |
|---|---|
| `ack` | `client_oid`, `exchange_oid`, `status`, `ts_ns` |
| `batch_ack` | `results: [Ok(OrderAck) \| Err(OrderError)]` |
| `cancel_count` | `count: u32` |
| `heartbeat_ok` | `next_due_ms: u64` |

### `OrderError` variants (tagged by `"kind"` field)

`risk_rejected` · `kill_switch` · `duplicate_client_oid` · `exchange_rejected` ·
`network` · `timeout` · `not_found` · `internal`

### Heartbeat / dead-man-switch

The engine sends `heartbeat` at a cadence shorter than `next_due_ms` (e.g.
every 5 s for a 15 s window). If the executor misses `2 × interval` it:
1. Calls `cancel_all` on every configured exchange.
2. Flips `risk:kill_switch = "true"` in Redis.

This is the top-level safety rail above all risk checks.

---

## Control PUB — `CONTROL_SOCKET`

Frame: `[topic_bytes, msgpack_payload]`

Subscribers connect with no topic filter (receive all). Publisher is `backend`
or a CLI tool.

### `ControlMessage` variants (tagged by `"type"` field)

| `type` | Fields | Effect |
|---|---|---|
| `shutdown` | — | All services stop cleanly |
| `pause` | `service: str` | Named service pauses |
| `resume` | `service: str` | Named service resumes |
| `strategy_params` | JSON value | Engine reloads strategy parameters |
| `terminate_strategy` | — | Engine stops current strategy |

---

## `libs/src/protocol/` module layout

| Module | Types |
|---|---|
| `mod.rs` | `ExchangeName`, `OrderbookSnapshot`, `PriceLevelTuple` |
| `events.rs` | `UserEvent`, `EventKind` |
| `orders.rs` | `OrderRequest`, `OrderResponse`, `OrderAction`, `OrderResult`, `OrderError`, `OrderAck`, `HeartbeatAck`, `PlaceOne`, `Side`, `OrderKind`, `Tif`, `OrderStatus` |
| `control.rs` | `ControlMessage` |

---

## Python mirror

The Python engine mirrors these types with `msgpack` + `pydantic` v2. Example:

```python
from typing import Literal, Annotated, Union
from pydantic import BaseModel

class PlaceOne(BaseModel):
    client_oid: str
    symbol: str
    side: Literal["buy", "sell"]
    kind: Literal["limit", "market"]
    px: float
    qty: float
    tif: Literal["GTC", "IOC", "FOK", "GTD"]

class PlaceAction(BaseModel):
    action: Literal["place"] = "place"
    client_oid: str
    symbol: str
    side: Literal["buy", "sell"]
    kind: Literal["limit", "market"]
    px: float
    qty: float
    tif: Literal["GTC", "IOC", "FOK", "GTD"]

class OrderRequest(BaseModel):
    req_id: int
    exchange: str
    action: dict   # discriminated by "action" key
```

Keep Python models in lockstep with `libs/src/protocol/` — any change to the
Rust enums must be reflected before deployment.
