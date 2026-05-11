# velociraptor

A high-performance Rust workspace for real-time market data streaming and order execution on prediction markets and crypto exchanges.

| Exchange | Status | Stream type |
|---|---|---|
| Binance USDT-M futures | Live | Partial Book Depth 20 @ 100ms |
| Binance Spot | Live | Partial Book Depth 20 @ 100ms + raw `@trade` |
| OKX | Live | `books` snapshot + incremental |
| Polymarket | Live | `book` snapshot + `price_change` diffs |
| Hyperliquid | Live | `l2Book` full snapshot every update |
| Kalshi | Live | `orderbook_snapshot` + signed `orderbook_delta` — RSA-PSS auth |

## Workspace

| Crate | Purpose |
|---|---|
| `libs` | Shared protocol types, configs, credentials, Redis client, terminal UI |
| `orderbook` | Multi-exchange market data engine (`StreamEngine`, `StreamSystem`, connectors) |
| `zmq_server` | ZMQ transport: PUB market data, ROUTER subscriptions, PUB user events |
| `recorder` | Append-only MessagePack writer with daily rotation + zstd |
| `executor` | ZMQ REP gateway for REST order placement (Kalshi, Polymarket CLOB v2) |
| `backend` | Axum HTTP API — reads Redis, serves market data over REST |
| `frontend` | React/Vite UI served by container nginx |

## Quick start

```bash
# Build
cargo build --release
cargo check --workspace

# Run the system via docker
make build         # builds shared builder + thin runtime images
make up            # docker compose up -d
make logs
make down

# Run from source
docker compose up -d redis
cargo run --bin orderbook_server --release -- --config configs/server.yaml
cargo run --bin backend          --release -- --config configs/server.yaml
cargo run --bin executor         --release -- --config configs/example.yaml \
    --credentials credentials/polymarket.yaml --skip-chmod-check
cd frontend && npm run dev
```

Container layout:
```
redis            -> 127.0.0.1:6379
backend          -> 127.0.0.1:3000
orderbook_server -> 127.0.0.1:5555 (PUB) / :5556 (ROUTER)
executor         -> 127.0.0.1:5557 (ROUTER) / :5558 (metrics)
frontend (nginx) -> 127.0.0.1:8080
```

## Minimal config (`configs/server.yaml`)

```yaml
server:  { pub_endpoint: "tcp://*:5555", router_endpoint: "tcp://*:5556" }
redis:   { enabled: true, url: "redis://127.0.0.1:6379",
           snapshot_cap: 100, trade_cap: 1000, event_list_cap: 5000 }
backend: { port: 3000 }
storage: { enabled: false, base_path: "./data", depth: 20,
           flush_interval: 1000, rotation: "daily", zstd_level: 0 }

binance:      { enabled: true, symbols: ["btcusdt", "ethusdt"] }
binance_spot: { enabled: true, symbols: ["btcusdt", "ethusdt"] }
okx:          { enabled: true, symbols: ["BTC-USDT", "ETH-USDT-SWAP"] }
hyperliquid:  { enabled: true, coins: ["BTC", "ETH"] }
kalshi:
  market:
    - { enable: true, series: "KXBTC15M" }
    - { enable: true, series: "KXETH15M" }
polymarket:
  markets:
    - { enabled: true, slug: "btc-updown-15m", interval_secs: 900 }
    - { enabled: true, slug: "eth-updown-15m", interval_secs: 900 }
```

`orderbook_server` CLI accepts `--config`, `--log-level`, `--log-json` only. All tuning lives in YAML.

## Architecture (one-liner)

Exchange WebSocket → `MsgParserTrait` → mpsc → `StreamEngine` (hooks + broadcast + `Orderbook.apply_update` copy-on-write) → consumers: ZMQ PUB / Redis / disk recorder.

## Reference

All deep documentation lives in project-scoped Claude skills under `.claude/skills/`. Load them by name when working in this repo:

| Topic | Skill |
|---|---|
| Project map, crates, run commands | `velociraptor-overview` |
| ZMQ sockets, topics, msgpack payloads, OrderAction | `velociraptor-zmq-protocol` |
| Python subscribers, user-event readers, order senders | `velociraptor-python-clients` |
| Rust StreamEngine / Orderbook / subscription builders | `velociraptor-rust-api` |
| Executor: ports, REST mapping, risk gates, audit, dead-man | `velociraptor-executor` |
| MessagePack file format, layouts, readers, archives | `velociraptor-storage` |
| Polymarket markets, recorder, Redis schema | `velociraptor-polymarket` |
| Kalshi auth, ticker, complement orderbook, scheduler | `velociraptor-kalshi` |
| Redis keys + HTTP backend endpoints | `velociraptor-backend-redis` |
| Step-by-step exchange connector guide | `velociraptor-add-exchange` |
| Per-exchange raw wire formats | `velociraptor-wire-formats` |
| Host nginx reverse proxy + HTTPS | `velociraptor-nginx` |
