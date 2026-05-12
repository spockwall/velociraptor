---
name: velociraptor-overview
description: Velociraptor workspace map, crate purposes, container/port layout, and pointers to the topic-specific skills. Load this when the user asks "what is this project" or wants a high-level orientation.
---

# Velociraptor — Project Overview

High-performance Rust workspace for real-time market data streaming and order execution on prediction markets and crypto exchanges.

## Supported exchanges

| Exchange | Stream type |
|---|---|
| Binance USDT-M futures | Partial Book Depth 20 @ 100ms |
| Binance Spot | Partial Book Depth 20 @ 100ms + raw `@trade` |
| OKX | `books` channel — snapshot + incremental |
| Polymarket | `book` snapshot + `price_change` diffs |
| Hyperliquid | `l2Book` full snapshot every update |
| Kalshi | `orderbook_snapshot` + signed `orderbook_delta` — RSA-PSS auth required |

## Workspace crates

| Crate | Purpose |
|---|---|
| `libs` | Shared protocol types, configs, credentials, Redis client, terminal UI |
| `orderbook` | Multi-exchange market data engine (`StreamEngine`, `StreamSystem`, connectors) |
| `zmq_server` | ZMQ transport: PUB (market data), ROUTER (subs), PUB (user events) |
| `recorder` | Append-only MessagePack snapshot writer with daily rotation and zstd |
| `executor` | ZMQ REP gateway for REST order placement (Kalshi, Polymarket CLOB v2) |
| `backend` | Axum HTTP API — reads Redis, exposes market data over REST |

## Container layout (docker compose)

```
redis            -> 127.0.0.1:6379
backend          -> 127.0.0.1:3000
orderbook_server -> 127.0.0.1:5555 (PUB) / :5556 (ROUTER)
executor         -> 127.0.0.1:5557 (ROUTER) / :5558 (metrics)
frontend (nginx) -> 127.0.0.1:8080
```

Three Rust services (backend, orderbook_server, executor) share one `Dockerfile.builder` image built via cargo-chef + BuildKit cache mounts. Each runtime Dockerfile is a thin debian-slim layer that COPYs its binary from the builder. Use `make build / up / logs / down / rebuild`.

## Where to find more

| Topic | Skill |
|---|---|
| Build / run / quick-start commands | this skill (below) |
| ZMQ sockets, topics, payloads, OrderAction | `velociraptor-zmq-protocol` |
| Python subscriber / order-sender examples | `velociraptor-python-clients` |
| Rust StreamEngine / StreamSystem / hooks | `velociraptor-rust-api` |
| Executor: ports, actions, risk gates, audit | `velociraptor-executor` |
| Recorder / MessagePack format / replay | `velociraptor-storage` |
| Polymarket markets, recorder, asset/price archives | `velociraptor-polymarket` |
| Kalshi auth, ticker format, scheduler | `velociraptor-kalshi` |
| Redis keys and HTTP backend endpoints | `velociraptor-backend-redis` |
| Adding a new exchange | `velociraptor-add-exchange` |
| Wire formats per exchange | `velociraptor-wire-formats` |
| Host nginx reverse-proxy setup | `velociraptor-nginx` |

## Top-level commands

```bash
# Build
cargo build --release
cargo check --workspace
cargo fmt
cargo clippy

# Servers
cargo run --bin orderbook_server --release -- --config configs/server.yaml
cargo run --bin backend          --release -- --config configs/server.yaml
cargo run --bin executor         --release -- --config configs/example.yaml \
    --credentials credentials/polymarket.yaml --skip-chmod-check

# Terminal visualisers
cargo run --example polymarket_orderbook --release
cargo run --example kalshi_orderbook     --release
cargo run --example orderbook            --release   # multi-exchange TUI

# Recorders
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml
cargo run --bin orderbook_recorder  --release -- --config configs/server.yaml

# Archives
cargo run --bin price_to_beat_fetcher --release -- --config configs/example.yaml
cargo run --bin asset_id_fetcher      --release -- --config configs/example.yaml
```

## Architecture (one-paragraph)

Exchange WebSocket → `ClientBase.handle_message` → `MsgParserTrait.parse_message` → `Vec<StreamMessage>` → mpsc → `StreamEngine` loop fires `HookRegistry` (sync, before apply) → broadcasts `OrderbookRaw` → calls `Orderbook.apply_update` (copy-on-write via `Arc::make_mut`) → fires hooks (after apply) → broadcasts `OrderbookSnapshot` → consumers (ZmqServer PUB, StorageWriter). Stored as `DashMap<"exchange:symbol", Arc<Orderbook>>`.

## Config layout (configs/server.yaml — minimal)

```yaml
server: { pub_endpoint: "tcp://*:5555", router_endpoint: "tcp://*:5556" }
redis:  { enabled: true, url: "redis://127.0.0.1:6379",
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

For Kalshi RSA credentials see `velociraptor-kalshi`.

## CLI flags accepted by `orderbook_server`

| Flag | Env | Default |
|---|---|---|
| `--config` | `CONFIG_FILE` | `configs/server.yaml` |
| `--log-level` | `LOG_LEVEL` | `info` |
| `--log-json` | `LOG_JSON` | `false` |

All other tuning lives in the YAML.
