---
name: velociraptor-backend-redis
description: Redis key schema written by orderbook_server and the Axum HTTP backend endpoints that read them. Use when working on the backend crate, Redis writes, or HTTP API.
---

# Velociraptor — Backend & Redis

## Redis keys (written by orderbook_server)

When `redis.enabled: true`, `orderbook_server` writes on every engine tick:

| Key pattern | Type | Contents |
|---|---|---|
| `ob:{exchange}:{symbol}` | string | Latest full orderbook snapshot (msgpack) |
| `bba:{exchange}:{symbol}` | string | Latest best-bid-ask (msgpack) |
| `snapshots:{exchange}:{symbol}` | list | Recent snapshots, capped at `snapshot_cap` |
| `trades:{exchange}:{symbol}` | list | Recent last-trade events, capped at `trade_cap` |
| `position:{exchange}:{symbol}` | string | Latest position (msgpack, from user channel) |
| `balance:{exchange}:{asset}` | string | Latest balance (msgpack, from user channel) |
| `events:fills` | list | Recent fill events, capped at `event_list_cap` |
| `events:orders` | list | Recent order updates, capped at `event_list_cap` |

All values are msgpack. Lists use `LPUSH + LTRIM` so index 0 = most recent.

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

`:exchange` matches lowercase enum name (`binance`, `binance_spot`, `okx`, `polymarket`, `hyperliquid`, `kalshi`). `:symbol` is exchange-native.

Missing keys → `{"error": "..."}` with status `404`. Decode errors → `500`.

Examples:

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/bba/binance/btcusdt
curl http://localhost:3000/api/orderbook/binance/btcusdt
curl "http://localhost:3000/api/snapshots/binance/btcusdt?limit=10"
curl "http://localhost:3000/api/trades/binance_spot/btcusdt?limit=5"
curl "http://localhost:3000/api/trades/polymarket/<asset_id>?limit=5"
```

## Backend heartbeat

Backend writes `executor:backend_heartbeat` unix-secs every 5s. Executor's dead-man fires if stale >30s — see `velociraptor-executor`.
