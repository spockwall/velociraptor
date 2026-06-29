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

# Method 1: Running in container by using makefile
# Run the whole stack via Docker. LABEL picks the env (prod|dev); default dev.
# `make up LABEL=<env>` selects configs/<env>/config.yaml + credentials/<env>/,
# and maps the host data root (prod -> /data, dev -> ./data) into /app/data in
# every container.
make builder
make build              # builds shared builder + thin runtime images
make up LABEL=prod      # production stack  (live CLOB, risk gate ON)
make up LABEL=dev       # dev stack         (testnet, risk gate OFF) — also the default
make logs
make down

# Method 2: Running raw building blocks
# Run individual binaries from source. The defaults target the DEV env, so a
# bare `cargo run` works against configs/dev/* + credentials/dev/*. Switch envs
# by passing `--config configs/prod/...` (see the binary→config table below).
docker compose up -d redis
cargo run --bin orderbook_server   --release    # --config configs/dev/config.yaml   (default)
cargo run --bin backend            --release    # --config configs/dev/config.yaml   (default)
cargo run --bin executor           --release -- --skip-chmod-check   # dev creds by default
cargo run --bin orderbook_recorder --release    # --config configs/dev/recorder.yaml (default)
cd frontend && npm run dev
```

**Dev vs prod.** `dev` runs against the Polymarket testnet with the pre-trade risk gate **off** and verbose logging; `prod` runs the live CLOB with the risk gate **on** and `info` logging. The two differ only in `configs/<env>/` and `credentials/<env>/` — same binaries, same code.

**Risk gate.** It's a standalone file the executor loads as the *sibling* of its `--config` (e.g. `configs/dev/config.yaml` → `configs/dev/risk.yaml`). Edit it and `SET executor:reload_config 1` (or click "reload risk" in the UI) to hot-reload without a restart. Prod ships `enabled: true`; dev ships off.

Container layout:
```
redis            -> 127.0.0.1:6379
backend          -> 127.0.0.1:3000
orderbook_server -> 127.0.0.1:5555 (PUB) / :5556 (ROUTER)
executor         -> 127.0.0.1:5557 (ROUTER) / :5558 (metrics)
frontend (nginx) -> 127.0.0.1:8080
```

## Config layout

Configs and credentials are organized per environment (`dev` / `prod`). Each `configs/<env>/` directory holds:

| File | Used by | Data paths |
|---|---|---|
| `config.yaml` | the long-running services — `orderbook_server`, `backend`, `executor` (the Docker stack) | container `/app/data` |
| `recorder.yaml` | the host recorder + fetcher binaries — `orderbook_recorder`, `price_to_beat_fetcher`, `asset_id_fetcher` | dev `./data`, prod `/data` |
| `risk.yaml` | the `executor` pre-trade gate — loaded as the sibling of its `--config`; hot-reloadable | — |
| `polymarket.yaml` | `polymarket_recorder` (prod: `enabled: true` → `/data/polymarket`) + the example/visualiser binaries (dev: `enabled: false`) | prod `/data/polymarket` |
| `kalshi.yaml` | the Kalshi example/visualiser binary | dev `./data` |

Credentials live in `credentials/<env>/{polymarket,kalshi}.yaml` (gitignored; `credentials/<env>/example.yaml` is the committed template).

Path rule: inside a container everything is `/app/data` (the host data root is bind-mounted there by Docker); on the host it's `./data` (dev) or `/data` (prod).

### Binary → config / credentials

Every binary defaults to the **dev** files, so a bare `cargo run` works in dev. Pass `--config configs/prod/…` (and the matching `--credentials`) to target prod.

| Binary | `--config` default | Credentials |
|---|---|---|
| `orderbook_server` | `configs/dev/config.yaml` | `--polymarket-credentials`, `--kalshi-credentials` → `credentials/dev/*` (optional: only for the user channel / authed Kalshi WS) |
| `backend` | `configs/dev/config.yaml` | — |
| `executor` | `configs/dev/config.yaml` (+ sibling `risk.yaml`) | `--credentials`, `--kalshi-credentials` → `credentials/dev/*` |
| `orderbook_recorder` | `configs/dev/recorder.yaml` | — |
| `price_to_beat_fetcher` | `configs/dev/recorder.yaml` | — |
| `asset_id_fetcher` | `configs/dev/recorder.yaml` | — |
| `polymarket_recorder` | `configs/dev/polymarket.yaml` (prod systemd: `configs/prod/polymarket.yaml` → writes `/data/polymarket`) | reads `credentials/<env>/polymarket.yaml` |
| examples (`polymarket_orderbook`, `polymarket_user_channel`, `polymarket_last_trade`) | `configs/dev/polymarket.yaml` | — |
| example (`kalshi_orderbook`) | `configs/dev/kalshi.yaml` | `credentials/<env>/kalshi.yaml` |

The Docker stack overrides these defaults via `make up LABEL=<env>`, which points each service at `configs/<env>/config.yaml` + `credentials/<env>/`. The systemd units (host, prod) override them to `configs/prod/recorder.yaml`.

### Minimal config (`configs/<env>/config.yaml`)

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

`orderbook_server` accepts `--config` (env `CONFIG_FILE`), `--polymarket-credentials` (env `POLYMARKET_CREDENTIALS_FILE`), and `--kalshi-credentials` (env `KALSHI_CREDENTIALS_FILE`). All tuning — including `logging.level` / `logging.json` — lives in the YAML.

## Run as services (systemd, Linux)

Four long-running binaries — the Polymarket recorder, orderbook recorder, price-to-beat fetcher, and asset-id fetcher — ship with systemd units under `deploy/systemd/`. They auto-start on boot (once enabled) and restart on crash. All host paths live under `/data`. Three of them (orderbook recorder + both fetchers) run with `--config configs/prod/recorder.yaml` and write under `/data/orderbook`, `/data/asset_ids`, `/data/price_to_beat`. The **Polymarket recorder** uses its own `--config configs/prod/polymarket.yaml` so it writes to a separate tree, `/data/polymarket/{slug}/…`.

```bash
# On the Linux server. Units run as user `ben` from /home/ben/velociraptor
# (edit the unit files if yours differ).

# 1. Pre-create the /data dirs the prod configs write to, owned by ben
sudo mkdir -p /data/syslog /data/orderbook /data/polymarket /data/asset_ids /data/price_to_beat
sudo chown -R ben:ben /data/syslog /data/orderbook /data/polymarket /data/asset_ids /data/price_to_beat

# 2. Build (clean env — an active conda/venv can contaminate the binary)
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  bash -c 'cd /home/ben/velociraptor && cargo build --release'

# 3. Install + enable + start
sudo cp deploy/systemd/*.service deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now \
  velociraptor-polymarket-recorder.service \
  velociraptor-orderbook-recorder.service \
  velociraptor-price-to-beat-fetcher.service \
  velociraptor-asset-id-fetcher.service

# Logs / status (ben can read its own services' logs — no sudo)
journalctl -u velociraptor-orderbook-recorder.service -f

# Update after a code change:
deploy/systemd/update.sh
```

See `deploy/systemd/README.md` for the full install, the unit→command map, reboot/crash-recovery behavior, and the upgrade workflow.

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
