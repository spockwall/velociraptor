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

When `redis.enabled: true`, every window publishes live state. Architecture: a **scheduler** owns window lifecycle, a **per-window engine** ingests both UP and DOWN WS streams (needed for full orderbook materialisation), and a forward hook on that engine writes Redis + publishes to the bus **for the UP side only** (the DOWN token is a mirror image of UP: buying YES at p = selling NO at 1-p).

### Key schema

Storage keys are keyed by **`base_slug`**, not `asset_id`. The same key is overwritten by every snapshot, including across window rollovers — the card/subscriber sees a continuous time-series.

| Key | Type | Lifetime | Written by |
|---|---|---|---|
| `ob:polymarket:{base_slug}` | string (msgpack) | overwritten each tick | per-window forward hook (UP only) |
| `bba:polymarket:{base_slug}` | string (msgpack) | overwritten each tick | per-window forward hook (UP only) |
| `snapshots:polymarket:{base_slug}` | list (msgpack) | `LPUSH + LTRIM` to `snapshot_cap` | per-window forward hook (UP only) |
| `trades:polymarket:{base_slug}` | list (msgpack) | `LPUSH + LTRIM` to `trade_cap` | per-window forward hook (UP only) |
| `polymarket:label:{asset_id}` | hash | one per live (asset, side) | window setup |
| `polymarket:label:index` | set of `asset_id` | mirrors live labels | window setup |
| `polymarket:base:{base_slug}:assets` | set of `asset_id` | per base slug | window setup |

The current window's per-window identifier is carried INSIDE the msgpack payload as `OrderbookSnapshot.full_slug: Option<String>` (and the same on `BbaPayload` / `LastTradePrice`). Subscribers detect rollover from the payload — no resubscribe needed.

**Do not** call `attach_redis(&mut engine, ...)` on per-window engines. That generic hook keys by `snap.symbol` (= asset_id) and would re-introduce the stale-asset_id storage problem. It's attached only to the main engine, which handles static exchanges where `symbol` IS the stable id.

`polymarket:label:{asset_id}` hash:

```
base_slug      = "btc-updown-15m"
full_slug      = "btc-updown-15m-1776683700"
side           = "up" | "down"
window_start   = "1776683700"     # parse from full_slug's trailing timestamp
interval_secs  = "900"            # 0 = static, never expires
```

> **`window_start` invariant.** Must parse from `full_slug`, **not** from `now`. Window setup runs ~10s before the previous window ends; `now / interval * interval` would yield the *current* window's start and the new window would be flagged stale immediately.

### Window setup (`spawn_polymarket_window`, `zmq_server/src/setup.rs`)

When scheduler fires for a new `full_slug`:

1. Resolve token IDs via Gamma → `Vec<(asset_id, base_slug, full_slug, is_up)>`.
2. **Evict expired prior-window LABELS** for this `base_slug`. For each `asset_id` in the base-slug set NOT in the freshly resolved set: read label hash; treat stale if `interval_secs==0` or `window_start+interval_secs<=now`. If stale: `DEL polymarket:label:{asset_id}`, `SREM polymarket:label:index`, `SREM polymarket:base:{base_slug}:assets`. **Active overlapping windows are skipped** (owned by their own task).
3. **No per-asset_id ob/bba/snapshots/trades deletes.** Those keys live at `base_slug` granularity and are overwritten by the next window's first snapshot.
4. Register new assets under `polymarket:base:{base_slug}:assets`.
5. Write `polymarket:label:{asset_id}` per new asset, add to `polymarket:label:index`.
6. Spin up a per-window `StreamEngine`. Register a forward hook on `OrderbookSnapshot` / `LastTradePrice` that:
   - Skips the frame if `is_up_by_asset.get(&snap.symbol) != Some(true)`.
   - Stamps `snap.full_slug = Some(full_slug.clone())`.
   - `bus.publish(StreamEvent::RollingSnapshot { base_slug, snap })` for ZMQ.
   - Writes the same payload to Redis: `set_orderbook("polymarket", &base_slug, ...)`, `lpush_capped(RedisKey::snapshots(...), ...)`, `set_bba(...)`. Trade-hook parallels.
7. Spawn a watchdog task polling `SystemControl.is_shutdown()`. On shutdown it removes only the LABELS this window owned (label hash + index + base-slug set membership). The ob/bba/snapshots/trades stay — they're still live data the next window will reuse.

### Backend read path (`GET /api/polymarket/markets`)

1. Read `polymarket:label:index`.
2. For each asset, load label hash. Empty → orphan, SREM and skip.
3. **Lazy expiry:** if `interval_secs > 0 && window_start + interval_secs <= now` → `DEL polymarket:label:{asset_id}` + `SREM polymarket:label:index`. Backstop for crashed processes that never ran their watchdog. Does NOT touch ob/bba/snapshots/trades.
4. Returns one row per (asset_id, side). Frontend filters `side === "up"` so each market shows as one card keyed by `base_slug`.
5. Sort by `(base_slug, window_start, side)`.

### Cleanup paths

| Path | Trigger | Owner | Affects |
|---|---|---|---|
| Eviction in window setup | New window starts | Scheduler | Stale LABELS only (storage keys are reused) |
| Watchdog | `SystemControl.shutdown()` | Per-window task | LABELS this window owned |
| Backend lazy expiry | API hit on stale label | Backend `expire_label` | LABEL hash + index-set membership |

No path tears down `ob:polymarket:{base_slug}` etc. They live as long as the base_slug is configured. If the orderbook server crashes mid-stream, the last snapshot stays in Redis for post-mortem inspection.

Steady-state invariant:

```
SCARD polymarket:label:index == 2 × (# enabled rolling markets)   # up + down
COUNT polymarket:label:*     == SCARD polymarket:label:index
COUNT ob:polymarket:*        == # enabled rolling markets (one per base_slug, UP only)
COUNT bba:polymarket:*       == # enabled rolling markets
```

### Health check

```bash
docker compose exec -T redis sh -c '
echo "--- labels ---"; redis-cli SCARD polymarket:label:index
echo "--- base_slug-keyed orderbook keys (expect one per market) ---"
redis-cli --scan --pattern "ob:polymarket:*"
echo "--- legacy asset_id-keyed keys (expect none) ---"
redis-cli --scan --pattern "ob:polymarket:*" | awk -F: '\''{print $3}'\'' | grep -E "^[0-9]{60,}$" || echo "(none)"
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
