---
name: velociraptor-polymarket
description: Polymarket integration — market mechanics, rolling Up/Down markets, token-id resolution via Gamma API, recorder, visualiser, Redis key schema, and the price-to-beat / asset-id archives. Use when working on anything Polymarket-specific.
---

# Velociraptor — Polymarket

For the on-the-wire message format see `velociraptor-wire-formats`. For storage layout / file formats / readers see `velociraptor-storage`.

## Market mechanism

Decentralised prediction market with YES/NO outcome tokens priced $0–$1 (implied probability). At resolution: winning token → $1, loser → $0.

**Rolling Up/Down markets** (this project's focus, e.g. `btc-updown-5m`, `btc-updown-15m`): asks whether asset goes up or down over a fixed window, then resets. Each side (Up / Down) is a unique 256-bit Polymarket `clobTokenId`. **Token IDs change every window** — must be resolved before subscribing.

Slug format: `{base_slug}-{unix_timestamp_of_window_start}`, e.g. `btc-updown-5m-1775315700`. Static (non-rolling) markets use slug directly with no timestamp suffix.

## Token resolution

Before each window, GET the Gamma REST API:

```
GET https://gamma-api.polymarket.com/markets/slug/{slug}
```

`clobTokenIds[0]` = Up/Yes token, `[1]` = Down/No. Synchronous, before WS opens. 404 → market not open yet, retry on next rotation.

## WebSocket

- **Endpoint:** `wss://ws-subscriptions-clob.polymarket.com/ws/market`
- **Always subscribe to both Up and Down token IDs together.** Subscribing to only one causes `price_change` events for the other to arrive before its snapshot → "update before snapshot" warnings.
- **Ping:** none (standard WS keep-alive).
- **Frames:** every server message is wrapped in a JSON array `[{...}]`.

## Configuration

```yaml
polymarket:
  markets:
    - { enabled: true, slug: "btc-updown-5m",  interval_secs: 300 }
    - { enabled: true, slug: "eth-updown-5m",  interval_secs: 300 }
    # - { enabled: true, slug: "will-btc-reach-100k-in-2025", interval_secs: 0 }   # static
```

| Field | Type | Description |
|---|---|---|
| `enabled` | bool | Skip if false |
| `slug` | str | Base slug from Polymarket URL |
| `interval_secs` | u64 | Window seconds. `0` = static market |

## Running

### Visualiser

```bash
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.yaml
# or
cargo run --example polymarket_orderbook --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --slug eth-updown-5m --interval-secs 300 --depth 8
```

### Recorder

Writes every snapshot to disk immediately (not at render rate):

```bash
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml
# or
cargo run --bin polymarket_recorder --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --base-path ./data --depth 10 --zstd-level 3
```

Storage section in the YAML:

```yaml
server:  { render_interval: 300 }
storage: { depth: 10, base_path: "./data", flush_interval: 1000, zstd_level: 3 }
```

### Window lifecycle (recorder)

1. Window opens → resolve token IDs via Gamma → open Up + Down `.mpack` files.
2. Every snapshot event → write immediately (typically 100–500ms cadence).
3. Every second → `BufWriter` flush.
4. `EARLY_START_SECS=10` before window end → next window's task pre-started (overlap).
5. Window expires → old task stopped, writers flushed/closed, zstd compression in background.
6. New window's task becomes sole writer.

## Redis integration (orderbook_server)

When `redis.enabled: true`, every window publishes live state. The architecture has a **scheduler** owning window lifecycle and a **per-window engine** owning Redis writes/cleanup.

### Key schema

| Key | Type | Lifetime | Written by |
|---|---|---|---|
| `ob:polymarket:{asset_id}` | string (msgpack) | overwritten each tick | per-window snapshot hook |
| `bba:polymarket:{asset_id}` | string (msgpack) | overwritten each tick | per-window snapshot hook |
| `snapshots:polymarket:{asset_id}` | list (msgpack) | `LPUSH + LTRIM` to `snapshot_cap` | per-window snapshot hook |
| `trades:polymarket:{asset_id}` | list (msgpack) | `LPUSH + LTRIM` to `trade_cap` | per-window trade hook |
| `polymarket:label:{asset_id}` | hash | one per live asset | window setup |
| `polymarket:label:index` | set of `asset_id` | mirrors live labels | window setup |
| `polymarket:base:{base_slug}:assets` | set of `asset_id` | per base slug | window setup |

`polymarket:label:{asset_id}` hash:

```
base_slug      = "btc-updown-15m"
full_slug      = "btc-updown-15m-1776683700"
side           = "up" | "down"
window_start   = "1776683700"     # parse from full_slug's trailing timestamp
interval_secs  = "900"            # 0 = static, never expires
```

> **`window_start` invariant.** Must parse from `full_slug`, **not** from `now`. Window setup runs ~10s before the previous window ends; `now / interval * interval` would yield the *current* window's start and the new window would be flagged stale immediately.

### Window setup (`spawn_polymarket_window`)

When scheduler fires for a new `full_slug`:

1. Resolve token IDs via Gamma → `Vec<(asset_id, base_slug, full_slug, is_up)>`.
2. **Evict expired prior-window keys** for this `base_slug`. For each `asset_id` in the base-slug set NOT in the freshly resolved set: read label, treat stale if `interval_secs==0` or `window_start+interval_secs<=now`. If stale: DEL label/ob/bba/snapshots/trades; SREM from index and base set. **Active overlapping windows are skipped** (owned by their own task).
3. Register new assets under `polymarket:base:{base_slug}:assets`.
4. Write `polymarket:label:{asset_id}` per new asset, add to `polymarket:label:index`.
5. Spin up a **per-window** `StreamEngine` (separate from main) and `attach_redis(...)`.
6. Spawn a **watchdog task** polling `SystemControl.is_shutdown()`. `WindowTask::stop()` → `handle.abort()` cancels anything awaiting past it — cleanup CANNOT live inline after `system.run().await`. The watchdog runs independently and DEL/SREMs every key the window owned.

### Backend read path (`GET /api/polymarket/markets`)

1. Read `polymarket:label:index`.
2. For each asset, load label hash. Empty → orphan, SREM and skip.
3. **Lazy expiry:** if `interval_secs > 0 && window_start + interval_secs <= now` → DEL all keys, remove from index. Backstop for crashed processes.
4. Sort by `(base_slug, window_start, side)` and return.

### Cleanup paths

| Path | Trigger | Owner | Purpose |
|---|---|---|---|
| Eviction in window setup | New window starts | Scheduler | Remove previous window's keys at rollover |
| Watchdog | `SystemControl.shutdown()` | Per-window task | Clean up when scheduler stops this window |
| Backend lazy expiry | API hit on stale label | Backend handler | Backstop if process killed before watchdog ran |

Steady-state invariant:

```
COUNT polymarket:label:* == SCARD polymarket:label:index
                         == COUNT ob:polymarket:*
                         == COUNT bba:polymarket:*
                         == COUNT snapshots:polymarket:*
                         == COUNT trades:polymarket:*
```

Live keys = `2 × (# enabled rolling markets)` + transient overlap of one extra window during pre-start.

### Health check

```bash
docker compose exec -T redis sh -c '
labels=$(redis-cli SMEMBERS polymarket:label:index | sort)
for prefix in ob bba snapshots trades; do
  ids=$(redis-cli --scan --pattern "$prefix:polymarket:*" | sed "s|$prefix:polymarket:||" | sort)
  echo "--- orphan $prefix (no label) ---"; comm -23 <(echo "$ids") <(echo "$labels")
done
echo "--- label has no ob ---"
obs=$(redis-cli --scan --pattern "ob:polymarket:*" | sed "s|ob:polymarket:||" | sort)
comm -13 <(echo "$obs") <(echo "$labels")
'
```

## Archives

**Price-to-Beat** (`priceToBeat` / `finalPrice` from Chainlink) and **Asset-ID** (per-window `market_id` + token IDs) — see `velociraptor-storage` for CSV schema and reader recipes. Why the 1-hour lookback: `priceToBeat` is only populated after the Chainlink start tick is observed; `finalPrice` only after the close tick. Both binaries default to `--lookback-secs 3600`.

Run examples:

```bash
# Fetcher (long-running daemon, all enabled markets)
cargo run --release --bin price_to_beat_fetcher -- --config configs/example.yaml

# One-shot backfill
cargo run --release --bin price_to_beat_backfill -- polymarket \
    --base-slug btc-updown-15m --interval-secs 900 \
    --from 2026-04-25T00:00:00Z

# Asset IDs
cargo run --release --bin asset_id_fetcher -- --config configs/example.yaml \
    --seed-from 2026-04-10T00:00:00Z --archive-dir ./data/asset_ids
```
