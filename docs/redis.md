# Redis usage in Velociraptor

This document is the authoritative map of every Redis key the workspace touches, who writes it, who reads it, and which Redis data type holds the value. Schema constants live in [`libs/src/redis_client/keys.rs`](../libs/src/redis_client/keys.rs); the single shared helper lives in [`libs/src/redis_client/mod.rs`](../libs/src/redis_client/mod.rs).

## TL;DR

- **Single shared instance** at `redis://127.0.0.1:6379` (host) or `redis://redis:6379` (docker network). Configured by `redis.url` in the unified YAML (`configs/example.yaml` / `example.docker.yaml`).
- **Modes used:** plain strings, hashes, lists (capped), sets, and one stream (`executor:log`). **No** pub/sub channels are live today (`executor:kill_switch_chan` and `config:updates` are reserved names but nothing subscribes).
- **No persistence.** docker-compose runs Redis with `--save "" --appendonly no` — Redis is treated as an ephemeral cache. Restarting Redis wipes every key listed below.
- **Payloads** are mostly `rmp-serde` (msgpack with named fields). A few control keys are plain UTF-8 strings ("1" / unix-seconds).
- **Writers:** the `orderbook_server` (via `zmq_server::setup::attach_redis`) produces all market and user-event keys; the `executor` produces all control-plane and audit keys.
- **Readers:** the `backend` reads everything for the HTTP API; the `executor` watches its own control keys.

## Connection

All services use the same helper:

```rust
use libs::redis_client::RedisHandle;

let handle = RedisHandle::connect(
    "redis://127.0.0.1:6379",
    /* event_list_cap = */ 5000,
).await?;
```

`RedisHandle` wraps `redis::aio::ConnectionManager` so reconnects on transient failures are automatic. Every write goes through one of:

| Helper                                        | Redis op           | Used for                          |
|-----------------------------------------------|--------------------|-----------------------------------|
| `set_orderbook`, `set_bba`, etc.              | `SET`              | Latest-snapshot strings           |
| `get_raw(key)`                                | `GET`              | Latest-snapshot strings           |
| `hset_multi(key, &[(field, value)])`          | `HSET`             | Label hashes                      |
| `hgetall(key)`                                | `HGETALL`          | Label hashes                      |
| `sadd`, `srem`, `smembers`                    | `SADD`/`SREM`/`SMEMBERS` | Label-index sets            |
| `lpush_capped(key, payload, cap)`             | `LPUSH` + `LTRIM`  | Capped event/snapshot/trade lists |
| `lrange_raw(key, start, stop)`                | `LRANGE`           | Reading capped lists              |
| `del(key)`                                    | `DEL`              | Expiry / one-shot triggers        |

All `set_*` and `get_raw` payloads are **msgpack** (`rmp-serde::to_vec_named`).

## Key namespace

### 1. Market data — written by `orderbook_server`

Written by `zmq_server::setup::attach_redis` hooks; see [`zmq_server/src/setup.rs`](../zmq_server/src/setup.rs).

| Key | Type | Payload | Writer | Reader | TTL |
|---|---|---|---|---|---|
| `ob:{exchange}:{symbol}` | string | msgpack `OrderbookSnapshot` (full depth) | engine hook on every `StreamEvent::OrderbookSnapshot` | `backend GET /api/orderbook/{ex}/{sym}` | none |
| `bba:{exchange}:{symbol}` | string | msgpack `BbaPayload` (best bid/ask + spread) | engine hook on every snapshot | `backend GET /api/bba/{ex}/{sym}` | none |
| `snapshots:{exchange}:{symbol}` | list (LPUSH newest-first) | msgpack `OrderbookSnapshot` per entry, capped at `redis.snapshot_cap` (default 100) | `lpush_capped` on every snapshot | `backend GET /api/snapshots/{ex}/{sym}?limit=N` | none |
| `trades:{exchange}:{symbol}` | list (LPUSH newest-first) | msgpack `LastTradePrice` per entry, capped at `redis.trade_cap` (default 1000) | `lpush_capped` on every `StreamEvent::LastTradePrice` | `backend GET /api/trades/{ex}/{sym}?limit=N` | none |

`{exchange}` is the lowercase `ExchangeName::to_str()` (`binance`, `binance_spot`, `okx`, `polymarket`, `hyperliquid`, `kalshi`). `{symbol}` is exchange-native (e.g. `BTCUSDT`, the Polymarket token id).

**Read example:**

```bash
# Latest BBA snapshot for one Polymarket token (msgpack — pipe through a decoder).
redis-cli --no-raw GET "bba:polymarket:60105229286427884692660113868141858131134689149752564702347657042086215753173" | xxd | head
```

```python
import msgpack, redis
r = redis.Redis(decode_responses=False)
raw = r.get(b"bba:polymarket:60105...")
bba = msgpack.unpackb(raw, raw=False)
# {'exchange': 'polymarket', 'symbol': '60105...', 'best_bid': [0.49, 1234.0], 'best_ask': [0.51, 856.0], ...}
```

### 2. Market labels — Polymarket + Kalshi rolling windows

Written by `spawn_polymarket_window` / `spawn_kalshi_window` in `zmq_server/src/setup.rs`. The label maps `asset_id` → human metadata so the backend can render readable titles in `GET /api/polymarket/markets` and `GET /api/kalshi/markets`.

| Key | Type | Fields / members | Writer | Reader |
|---|---|---|---|---|
| `polymarket:label:{asset_id}` | hash | `base_slug`, `full_slug`, `side` (`up`/`down`), `window_start` (unix s), `interval_secs` | per-window task on start | `backend::routes::markets::get_polymarket_markets` |
| `polymarket:label:index` | set | every active `asset_id` | per-window task on start | backend (then iterates the hashes) |
| `polymarket:base:{base_slug}:assets` | set | asset_ids belonging to the active window for one base slug | per-window task on start | per-window task (evicts prior window) |
| `kalshi:label:{ticker}` | hash | `series`, `ticker`, `window_start`, `interval_secs` | per-window task on start | `backend::routes::markets::get_kalshi_markets` |
| `kalshi:label:index` | set | every active ticker | per-window task on start | backend |
| `kalshi:series:{series}:tickers` | set | tickers for the active window of a series | per-window task on start | per-window task (evicts prior window) |

The backend's `get_polymarket_markets` / `get_kalshi_markets` routes auto-evict any label whose `window_start + interval_secs <= now` and return only currently-active windows. They also clean up the associated `ob:*`, `bba:*`, `snapshots:*`, `trades:*` keys for the expired asset.

**Read example:**

```bash
# All active Polymarket assets.
redis-cli SMEMBERS polymarket:label:index

# Metadata for one.
redis-cli HGETALL polymarket:label:60105229286...
# 1) "base_slug"
# 2) "btc-updown-15m"
# 3) "full_slug"
# 4) "btc-updown-15m-1715423400"
# 5) "side"
# 6) "up"
# ...
```

### 3. User events — written by `orderbook_server`, consumed by `backend`

The Polymarket WS user channel feeds `StreamEvent::User(...)` into the engine; `attach_redis`'s user-event hook splits the two variants:

| UserEvent variant | Redis target | Writer call | Cap |
|---|---|---|---|
| `Fill` | list `events:fills` (LPUSH newest-first) | `lpush_capped` | `redis.event_list_cap` (default 5000) |
| `OrderUpdate` | list `events:orders` (LPUSH newest-first) | `lpush_capped` | same |

Polymarket's WS user channel does not push balance or position events — those would require a REST poller, which we don't run today.

| Key | Type | Payload | Reader |
|---|---|---|---|
| `events:fills` | list | msgpack `UserEvent::Fill` per entry | `backend GET /api/events/fills?limit=N` |
| `events:orders` | list | msgpack `UserEvent::OrderUpdate` per entry | `backend GET /api/events/orders?limit=N` |
| `events:log` | list | reserved — no writer today | — |

The frontend (`Account → Fills` / `Account → Orders`) polls the backend routes every ~2s; the backend's `events.rs` route does `LRANGE 0 N-1` and msgpack-decodes each entry into JSON.

**Read example:**

```bash
# Latest 5 fills, decoded inline via Python.
redis-cli LRANGE events:fills 0 4 | python3 -c "import sys, msgpack
for line in sys.stdin.buffer:
    try: print(msgpack.unpackb(line, raw=False))
    except: pass"
```

### 4. Executor control plane — written by `backend`, watched by `executor`

The executor's watcher task (`executor/src/control/mod.rs::run_watcher`) polls four keys every 250ms. The backend (or any operator) writes to them to engage kill-switch / cancel-all behaviors.

| Key | Type | Value | Writer | Reader |
|---|---|---|---|---|
| `executor:kill_switch` | string | `"1"` = engaged, anything else = off | manual / backend | executor (polls @ 250ms) |
| `executor:kill_switch:{exchange}` | string | `"1"` = engaged for one exchange (e.g. `executor:kill_switch:polymarket`) | manual / backend | executor |
| `executor:cancel_all` | string | `"1"` = one-shot trigger. Executor fires `cancel_all` on every client and `DEL`s the key after | manual / backend | executor |
| `executor:backend_heartbeat` | string | unix seconds. Stale > 30s ⇒ executor auto-engages local kill | backend (should be every 5s — not wired today) | executor |
| `executor:log` | **stream** | XADD entries `{payload: msgpack(AuditEntry)}`, MAXLEN ~ `executor.audit_stream_cap` (default 100k) | executor `AuditSink` on every request/response/synthetic event | external tooling (XRANGE / XREAD) |
| `executor:kill_switch_chan` | (reserved — no live publishers / subscribers) | — | — | — |

The executor crate-internal flags (`ControlState::kill_switch`, `per_exchange_kill`, `deadman_engaged`) mirror these keys in-process so the gateway can answer "is this exchange blocked?" without a Redis round-trip per order.

**Operator examples:**

```bash
# Engage global kill — every Place rejected until you DEL it.
redis-cli SET executor:kill_switch 1

# Polymarket-only kill.
redis-cli SET executor:kill_switch:polymarket 1

# Trigger cancel-all on every client (one-shot).
redis-cli SET executor:cancel_all 1

# Backend heartbeat (call every 5s from the backend; otherwise stale > 30s = local kill).
redis-cli SET executor:backend_heartbeat $(date +%s)

# Disengage everything.
redis-cli DEL executor:kill_switch executor:kill_switch:polymarket
```

**Read the audit stream:**

```bash
# Last 20 entries.
redis-cli XREVRANGE executor:log + - COUNT 20

# Live tail.
redis-cli XREAD COUNT 10 BLOCK 0 STREAMS executor:log '$'
```

Each entry is `{payload: <msgpack bytes>}`. Decoded:

```python
import msgpack, redis
r = redis.Redis(decode_responses=False)
for stream_id, fields in r.xrange("executor:log", "-", "+", count=20):
    entry = msgpack.unpackb(fields[b"payload"], raw=False)
    # {'seq': 12, 'prev_hash': 'a3f1...', 'ts_ns': ..., 'payload': {'kind': 'request', 'req': {...}}}
```

### 5. Reserved namespaces (defined, not yet wired)

Listed for completeness; no code reads or writes these today. Don't rely on them — they're placeholders for upcoming features.

| Key | Intended type | Intended purpose |
|---|---|---|
| `orders:open:{exchange}` | (set/hash TBD) | mirror of open orders per exchange |
| `target_price:polymarket:{slug}` | hash | strike/target prices for Polymarket markets |
| `target_price:kalshi:{ticker}` | hash | strike/target prices for Kalshi markets |
| `engine:status` | string | trading engine status |
| `engine:params` | hash | strategy params hot-reload |
| `risk:config` | hash | risk gate config (out-of-tree pre-refactor) |
| `risk:kill_switch` | string | risk-layer kill (superseded by `executor:kill_switch`) |
| `config:updates` | pub/sub channel | config hot-reload notifications |

## Operational notes

- **Persistence.** Disabled by design. Docker compose runs Redis with `--save "" --appendonly no`. Losing Redis loses everything in §1–4. The audit log file on disk (`./data/executor/{date}.mpack`) is the durable copy of executor activity.
- **Eviction policy.** `--maxmemory 512mb --maxmemory-policy noeviction`. Hitting the cap will start failing writes rather than dropping keys; caps on lists (`snapshot_cap`, `trade_cap`, `event_list_cap`) keep total usage bounded.
- **No TTLs.** Every key lives until something explicitly deletes it. Stale Polymarket / Kalshi labels are GC'd by `expire_label` inside the backend's market routes on every call. Static `ob:*` / `bba:*` keys are deleted only when their label gets expired or by manual cleanup.
- **Multiple writers per key are intentional and safe** — only one process writes each key (`orderbook_server` for market/user data; `executor` for control/audit; backend for heartbeat). No CAS or locking is used because the access pattern is single-writer per key.

## Cheat-sheet

```bash
# List all live Polymarket assets.
redis-cli SMEMBERS polymarket:label:index

# Resolve one asset → metadata.
redis-cli HGETALL polymarket:label:<asset_id>

# Latest BBA for an asset (binary; pipe to msgpack decoder).
redis-cli --no-raw GET bba:polymarket:<asset_id>

# Recent fills (binary list).
redis-cli LRANGE events:fills 0 9

# Engage / clear kill switch.
redis-cli SET executor:kill_switch 1
redis-cli DEL  executor:kill_switch

# Audit-log tail (binary stream).
redis-cli XREVRANGE executor:log + - COUNT 5
```
