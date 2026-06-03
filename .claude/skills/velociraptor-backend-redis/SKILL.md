---
name: velociraptor-backend-redis
description: Redis key schema written by orderbook_server and the Axum HTTP backend endpoints that read them. Use when working on the backend crate, Redis writes, or HTTP API.
---

# Velociraptor — Backend & Redis

## Redis keys (written by orderbook_server)

When `redis.enabled: true`, `orderbook_server` writes on every engine tick:

| Key pattern | Type | Contents |
|---|---|---|
| `ob:{exchange}:{slug}` | string | Latest full orderbook snapshot (msgpack) |
| `bba:{exchange}:{slug}` | string | Latest best-bid-ask (msgpack) |
| `snapshots:{exchange}:{slug}` | list | Recent snapshots, capped at `snapshot_cap` |
| `trades:{exchange}:{slug}` | list | Recent last-trade events, capped at `trade_cap` |
| `position:{exchange}:{symbol}` | string | Latest position (msgpack, from user channel) |
| `balance:{exchange}:{asset}` | string | Latest balance (msgpack, from user channel) |
| `events:fills` | list | Recent fill events, capped at `event_list_cap` |
| `events:orders` | list | Recent order updates, capped at `event_list_cap` |
| `system:metrics` | list | Host monitor samples (msgpack), capped 2880 ≈ 24h — written by the **backend** sampler, not orderbook_server (see "System monitor" below) |

`{slug}` is the STABLE identifier:
- Static exchanges (`binance`/`binance_spot`/`okx`/`hyperliquid`): exchange-native symbol (`btcusdt`, `BTC-USDT`).
- **Polymarket: `base_slug`** (e.g. `btc-updown-15m`) — NOT `asset_id`. Only the UP-side token is written; the down side is a mirror image and is dropped at the forward hook.
- **Kalshi: `series`** (e.g. `KXBTC15M`) — NOT `ticker`.

The rolling-market entry is overwritten by every snapshot, including across window rollovers. The current window's per-window identifier (`full_slug` for Polymarket, `ticker` for Kalshi) is carried inside the msgpack payload as `OrderbookSnapshot.full_slug: Option<String>` so subscribers can detect rollover from the payload without resubscribing.

All values are msgpack. Lists use `LPUSH + LTRIM` so index 0 = most recent.

Two write paths exist in `zmq_server/src/setup.rs` — picking the wrong one re-introduces an asset_id-keyed-storage bug:
1. `attach_redis(&mut engine, ...)` — generic hook keyed by `snap.symbol`. Attached ONLY to the **main engine** for static exchanges (`orderbook_server.rs:93`).
2. Per-window forward hook inside `spawn_polymarket_window` / `spawn_kalshi_window` — writes inline, keyed by `base_slug` / `series`. `attach_redis` is deliberately NOT called on per-window engines.

For Polymarket/Kalshi label/index/series keys see those skills (window lifecycle owns them).

## Config

```yaml
redis:
  enabled: true
  url: "redis://127.0.0.1:6379"
  snapshot_cap: 100     # per-symbol cap
  trade_cap: 1000
  event_list_cap: 5000  # events:fills / events:orders
```

Caps are per-key, not global. Memory scales with number of live keys.

Start Redis: `docker compose up redis -d`.

## HTTP backend (Axum)

Lightweight server that reads the Redis keys and exposes them as JSON.

```bash
cargo run --bin backend --release -- --config configs/example.yaml
```

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | `{"ok": true}` |
| `GET` | `/api/orderbook/:exchange/:symbol` | Latest full snapshot |
| `GET` | `/api/bba/:exchange/:symbol` | Latest BBA |
| `GET` | `/api/snapshots/:exchange/:symbol?limit=N` | Recent N snapshots (default 20) |
| `GET` | `/api/trades/:exchange/:symbol?limit=N` | Recent N trades (default 20) |
| `GET` | `/api/polymarket/markets` | Live Polymarket windows — see `velociraptor-polymarket` |
| `GET` | `/api/kalshi/markets` | Live Kalshi windows — see `velociraptor-kalshi` |
| `GET` | `/api/monitor` | Live host CPU / memory / disk + systemd unit status (no Redis read) |
| `GET` | `/api/monitor/history?limit=N` | Recent monitor samples, newest-first (default 720, max 2880) |

`:exchange` matches lowercase enum name (`binance`, `binance_spot`, `okx`, `polymarket`, `hyperliquid`, `kalshi`). `:symbol` is the stable slug: exchange-native symbol for static exchanges, **`base_slug` for Polymarket**, **`series` for Kalshi**.

Missing keys → `{"error": "..."}` with status `404`. Decode errors → `500`.

Examples:

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/bba/binance/btcusdt
curl http://localhost:3000/api/orderbook/binance/btcusdt
curl "http://localhost:3000/api/snapshots/binance/btcusdt?limit=10"
curl "http://localhost:3000/api/trades/binance_spot/btcusdt?limit=5"

# Rolling markets — use base_slug / series, NOT asset_id / ticker:
curl http://localhost:3000/api/orderbook/polymarket/btc-updown-15m
curl "http://localhost:3000/api/trades/polymarket/btc-updown-15m?limit=5"
curl http://localhost:3000/api/orderbook/kalshi/KXBTC15M
```

`/api/polymarket/markets` returns one row per (asset_id, side); callers wanting one card per market filter `side === "up"` client-side. `expire_label` in `backend/src/routes/markets.rs` only drops the label hash + index-set membership — ob/bba/snapshots/trades are reused across rollovers, so they're NOT torn down per label.

## System monitor

`backend/src/routes/monitor.rs::sample_loop` is a background task spawned by `bin/backend.rs`. Every 30s it collects a `MonitorStatus` (host CPU/mem/disk via the `sysinfo` crate + systemd unit state via `systemctl show`) and:

- LPUSH-es it (msgpack) onto the capped Redis list `system:metrics` (`keys::System::METRICS`, cap 2880 ≈ 24h),
- appends it as one JSON line to `{logging.dir}/system/YYYY-MM-DD.log` (durable record; Redis is volatile + capped).

`GET /api/monitor/history` reads the Redis list back; the frontend Monitor page charts CPU / memory / busiest-disk % over time (recharts `LineChart`). systemd units queried mirror `deploy/systemd/velociraptor.target`; on a host without systemd each unit reports `active_state: "unknown"` so the page still renders.

## Backend heartbeat

Backend writes `executor:backend_heartbeat` unix-secs every 5s. Executor's dead-man fires if stale >30s — see `velociraptor-executor`.
