# ZMQ Wire Protocol

All payloads are **msgpack** (`rmp-serde`). All IPC sockets live under `/tmp/trading/`. Socket constants in `libs::constants`; types in `libs/src/protocol/`.

## Socket map

| Constant | Path / endpoint | Pattern | Encoding |
|---|---|---|---|
| `MARKET_DATA_SOCKET` | `ipc:///tmp/trading/market_data.sock` (tcp `:5555`) | PUB/SUB | msgpack |
| `router_endpoint` | `tcp://*:5556` | ROUTER/DEALER | **JSON** |
| `WS_STATUS_SOCKET` | `ipc:///tmp/trading/ws_status.sock` | PUB/SUB | msgpack |
| `EXECUTOR_ORDER_SOCKET` | `ipc:///tmp/trading/executor_orders.sock` (tcp `:5557`) | REQ/REP | msgpack |
| `CONTROL_SOCKET` | `ipc:///tmp/trading/control.sock` | PUB/SUB | msgpack |
| `ENGINE_METRICS_SOCKET` | `ipc:///tmp/trading/engine_metrics.sock` | PUB/SUB | msgpack |

## Sockets at a glance

- **Market PUB** — topic `"{exchange}:{symbol}"`, payload `OrderbookSnapshot` or `BbaPayload` (selected via ROUTER subscribe `"type"`).
- **ROUTER** — JSON `{action: subscribe|unsubscribe, exchange, symbol, type}` → JSON `{status: ok|error}`. Control plane only.
- **User PUB** — topic `"user.{exchange}.{kind}"` (kind ∈ `fill`/`order_update`/`balance`/`position`), payload `UserEvent` tagged by `"type"`.
- **Executor REQ/REP** — msgpack `OrderRequest{req_id, exchange, action}` → `OrderResponse{req_id, result}`. `action` ∈ `place`, `place_batch`, `update`, `cancel`, `cancel_all`, `cancel_market`, `heartbeat`.
- **Control PUB** — `ControlMessage` tagged by `"type"` ∈ `shutdown`, `pause`, `resume`, `strategy_params`, `terminate_strategy`.

## Deep reference

Complete payload schemas, every `OrderAction` field, `OrderError` variants, ExchangeName enum encoding, Python pydantic mirrors, and the dead-man-switch flow are documented in the project skill **`velociraptor-zmq-protocol`** at `.claude/skills/velociraptor-zmq-protocol/SKILL.md`.

Related skills:
- `velociraptor-python-clients` — pyzmq + msgpack recipes
- `velociraptor-executor` — REST mapping, risk gates, audit
