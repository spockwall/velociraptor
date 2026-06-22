# Polymarket — Market Mechanism and Recording

## Market mechanism

Decentralised prediction market with YES/NO outcome tokens priced $0–$1 (implied probability). At resolution: winner → $1, loser → $0.

**Rolling Up/Down markets** (e.g. `btc-updown-5m`, `btc-updown-15m`) ask whether an asset rises or falls over a fixed window, then reset. Each side (Up/Down) is a unique 256-bit `clobTokenId` and **token IDs change every window**.

Slug format: `{base_slug}-{unix_timestamp_of_window_start}`. Static markets use slug directly with no suffix.

## Token resolution

```
GET https://gamma-api.polymarket.com/markets/slug/{slug}
```

`clobTokenIds[0]` = Up/Yes, `[1]` = Down/No. Synchronous, before WebSocket open. 404 → market not open yet, retry on next rotation.

**Always subscribe to both Up and Down tokens together** — solo subs cause `price_change` for the other side to arrive before its snapshot.

## Configuration

```yaml
polymarket:
  markets:
    - { enabled: true, slug: "btc-updown-5m", interval_secs: 300 }
    - { enabled: true, slug: "eth-updown-5m", interval_secs: 300 }
```

`interval_secs: 0` = static market.

## Running

```bash
# Visualiser
cargo run --example polymarket_orderbook --release -- --config configs/dev/polymarket.yaml

# Recorder (writes every snapshot immediately, not at render rate)
cargo run --bin polymarket_recorder --release -- --config configs/dev/polymarket.yaml
```

## File layout

```
{base_path}/{base_slug}/{YYYY-MM-DD}/
    {HH:MM}-{HH:MM}-up.mpack
    {HH:MM}-{HH:MM}-down.mpack
    {HH:MM}-{HH:MM}-up-trades.mpack
    {HH:MM}-{HH:MM}-down-trades.mpack
```

Format details: `docs/storage.md`. After window close, `.mpack` → `.mpack.zst` if `zstd_level > 0`.

## Redis integration

When `redis.enabled: true`, every window publishes live state. Scheduler owns window lifecycle, per-window engine owns Redis writes, watchdog handles cleanup.

Key schema:

| Key | Type |
|---|---|
| `ob:polymarket:{asset_id}` | string (msgpack) — overwritten each tick |
| `bba:polymarket:{asset_id}` | string (msgpack) — overwritten each tick |
| `snapshots:polymarket:{asset_id}` | list — LPUSH+LTRIM to `snapshot_cap` |
| `trades:polymarket:{asset_id}` | list — LPUSH+LTRIM to `trade_cap` |
| `polymarket:label:{asset_id}` | hash (`base_slug`, `full_slug`, `side`, `window_start`, `interval_secs`) |
| `polymarket:label:index` | set of `asset_id` |
| `polymarket:base:{base_slug}:assets` | set of `asset_id` |

> **`window_start` invariant** — must parse from `full_slug`'s trailing timestamp, NOT from `now`. Window setup runs ~10s before previous window ends; `now / interval * interval` would yield the *current* window's start and the new window would be flagged stale immediately.

Three cleanup paths: eviction at next rollover, per-window watchdog at shutdown, backend lazy expiry as a backstop.

Backend handler `GET /api/polymarket/markets` reads the index, drops stale/orphan entries, sorts by `(base_slug, window_start, side)`.

## Deep reference

Full architecture (lifecycle, eviction algorithm, cleanup invariant `COUNT label == COUNT ob == …`, health-check script, archives) is in the project skill **`velociraptor-polymarket`** at `.claude/skills/velociraptor-polymarket/SKILL.md`.

Related skills:

- `velociraptor-wire-formats` — Polymarket `book` / `price_change` payloads
- `velociraptor-storage` — file format, price-to-beat archive, asset-id archive
