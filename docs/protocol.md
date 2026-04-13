# ZMQ wire protocol

All messages on the trading system's ZMQ sockets are **msgpack** payloads
(`rmp-serde` on the Rust side, `msgpack` + `pydantic` on the Python side).
Sockets are IPC-only on `/tmp/trading/*.sock`; the trust boundary is the
machine, so there is no CURVE authentication.

The Rust source of truth lives at `libs/src/protocol/`. The Python engine
should mirror these types exactly.

## Sockets

| Constant (`libs::constants`)   | Path                                          | Pattern | Publisher          | Subscribers        |
|--------------------------------|-----------------------------------------------|---------|--------------------|--------------------|
| `MARKET_DATA_SOCKET`           | `ipc:///tmp/trading/market_data.sock`         | PUB/SUB | orderbook-server   | engine             |
| `WS_STATUS_SOCKET`             | `ipc:///tmp/trading/ws_status.sock`           | PUB/SUB | orderbook-server (user channel) | engine |
| `WS_ORDERS_SOCKET`             | `ipc:///tmp/trading/ws_orders.sock`           | PUB/SUB | orderbook-server, executor | engine |
| `CONTROL_SOCKET`               | `ipc:///tmp/trading/control.sock`             | PUB/SUB | backend / CLI      | all services       |
| `ENGINE_METRICS_SOCKET`        | `ipc:///tmp/trading/engine_metrics.sock`      | PUB/SUB | engine (Python)    | backend            |
| `EXECUTOR_ORDER_SOCKET`        | `ipc:///tmp/trading/executor_orders.sock`     | REQ/REP | engine             | executor           |

Topic frames (first ZMQ frame) precede the msgpack body on PUB/SUB sockets:

- market data: `md.{exchange}.{symbol}`
- user events: `user.{exchange}.{account|position|order_update|fill}`
- executor acks: `executor.result`
- engine metrics: `engine.{status|pnl|log}`
- control: `ctrl.{shutdown|pause.{service}|resume.{service}|strategy.params}`

## Order request/response (`EXECUTOR_ORDER_SOCKET`, REQ/REP)

```rust
OrderRequest  { req_id: u64, exchange: Exchange, action: OrderAction }
OrderResponse { req_id: u64, result: Result<OrderResult, OrderError> }
```

`Exchange` = `polymarket` | `kalshi`.

`OrderAction` variants (serde `tag = "action"`):

| `action`         | Fields                                                                |
|------------------|------------------------------------------------------------------------|
| `place`          | one `PlaceOne` (inline fields)                                         |
| `place_batch`    | `orders: [PlaceOne]`                                                   |
| `update`         | `client_oid`, `exchange_oid`, `new_px?`, `new_qty?`                    |
| `cancel`         | `exchange_oid`                                                         |
| `cancel_all`     | —                                                                      |
| `cancel_market`  | `symbol`                                                               |
| `heartbeat`      | —                                                                      |

`PlaceOne = { client_oid, symbol, side, kind, px, qty, tif }`
where `side ∈ {buy, sell}`, `kind ∈ {limit, market}`, `tif ∈ {GTC, IOC, FOK, GTD}`.

`OrderResult` variants (serde `tag = "result"`):

| `result`         | Fields                                                                 |
|------------------|------------------------------------------------------------------------|
| `ack`            | `OrderAck`                                                             |
| `batch_ack`      | `results: [Result<OrderAck, OrderError>]` (per-order outcomes)         |
| `cancel_count`   | `count: u32`                                                           |
| `heartbeat_ok`   | `HeartbeatAck { next_due_ms }`                                         |

`OrderAck = { client_oid, exchange_oid, status, ts_ns }` where `status ∈ {new, partially_filled, filled, canceled, rejected, expired}`.

`OrderError` variants (serde `tag = "kind"`): `risk_rejected`, `kill_switch`,
`duplicate_client_oid`, `exchange_rejected`, `network`, `timeout`,
`not_found`, `internal`.

### Heartbeat / dead-man-switch

The engine sends `OrderAction::Heartbeat` on a cadence shorter than
`next_due_ms` (e.g. every 5s for a 15s window). If the executor misses
`2 × interval` it will (1) call `cancel_all` on every configured exchange
and (2) flip the Redis key `risk:kill_switch` to `"true"`. This is the
top-level safety rail above the risk checks.

## User events (`WS_STATUS_SOCKET` / `WS_ORDERS_SOCKET`, PUB/SUB)

`UserEvent` variants (serde `tag = "type"`): `balance`, `position`,
`order_update`, `fill`. See `libs/src/protocol/events.rs` for exact
field layouts. Every event carries `exchange: String` and `ts_ns: i64`.

## Control (`CONTROL_SOCKET`, PUB/SUB)

`ControlMessage` variants (serde `tag = "type"`): `shutdown`, `pause`,
`resume`, `strategy_params` (carries a `serde_json::Value`),
`terminate_strategy`.

## Market data (`MARKET_DATA_SOCKET`, PUB/SUB)

Market-data payloads (`SnapshotPayload`, `BbaPayload`) live in the
`orderbook` crate at `orderbook/src/publisher/protocol.rs` and are
serialized by `ZmqPublisher`. They are not re-exported from
`libs::protocol` to avoid a circular dependency; the schema is stable
and documented in `docs/exchange-wire-formats.md` style elsewhere.

## Python mirror

The Python side uses `msgpack` + `pydantic` v2 models with the same
field names and variant tags. Example for `OrderRequest`:

```python
from typing import Literal
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
    # ...PlaceOne fields inlined by serde(flatten) — mirror inline

# etc. Full Python models live alongside the engine repo.
```

Keep the Python models in lockstep with `libs/src/protocol/` — any change
to the Rust enums must be reflected before deployment.
