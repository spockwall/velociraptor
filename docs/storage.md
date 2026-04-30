# Storage — MessagePack Snapshot Files

The recorder crate persists every orderbook snapshot to disk as an append-only binary file using the MessagePack format.

MessagePack is a binary serialization format that is:

- **Compact** — smaller than JSON, no field-name repetition per row
- **Fast** — no parsing overhead; a 100ms feed generates ~600k records/day per symbol
- **Schema-flexible** — add fields in new versions without breaking old readers
- **Language-agnostic** — first-class support in Python (`msgpack`), Rust (`rmp-serde`), and most other languages

Each record is serialized as a **msgpack map** (string keys → values), not an array, so field names are preserved and readable without a schema file.

---

## Wire Format

Each file is a flat sequence of length-prefixed msgpack records:

```
┌─────────────────────────────────────────────────────┐
│  Record 1                                           │
│  ┌──────────┬─────────────────────────────────────┐ │
│  │ 4 bytes  │  N bytes                            │ │
│  │ u32 LE   │  msgpack map                        │ │
│  │ (length) │  { "sequence": 1, "ts_ns": ... }    │ │
│  └──────────┴─────────────────────────────────────┘ │
│  Record 2                                           │
│  ┌──────────┬─────────────────────────────────────┐ │
│  │ 4 bytes  │  N bytes                            │ │
│  └──────────┴─────────────────────────────────────┘ │
│  ...                                                │
└─────────────────────────────────────────────────────┘
```

- The first 4 bytes of every record are a **little-endian u32** giving the byte length of the msgpack payload that follows.
- Records are written sequentially with no padding or index. Reading is always done front-to-back.

---

## Record Schema

Each record contains the following fields:

| Field      | Type           | Description                                   |
| ---------- | -------------- | --------------------------------------------- |
| `sequence` | u64            | Monotonically increasing update counter       |
| `ts_ns`    | i64            | UTC timestamp in nanoseconds since Unix epoch |
| `bids`     | `[[f64, f64]]` | Top-N bids as `[price, qty]`, best first      |
| `asks`     | `[[f64, f64]]` | Top-N asks as `[price, qty]`, best first      |

`N` is controlled by the `depth` setting in the config. `exchange`, `symbol`, `spread`, `mid`, and `wmid` are not stored — exchange and symbol are encoded in the file path, and the derived metrics are computed from `bids`/`asks` at read time.

---

## File Layout

### Standard exchanges (Binance, OKX, Hyperliquid)

Files are organised by exchange, symbol, and date:

```
{base_path}/
└── {exchange}/
    └── {SYMBOL}/
        ├── 2026-04-01.mpack
        ├── 2026-04-02.mpack
        ├── 2026-04-02.mpack.zst   ← compressed after daily rotation (if enabled)
        └── 2026-04-03.mpack
```

Examples:

```
data/binance/BTCUSDT/2026-04-03.mpack
data/hyperliquid/BTC/2026-04-03.mpack
data/okx/BTC-USDT/2026-04-03.mpack
```

### Binance Spot

Binance Spot collects both **partial-depth orderbook snapshots** and **raw trade events** (one record per matched fill). Both streams write into the same per-symbol directory; the trade file uses a `-trades.mpack` filename suffix:

```
{base_path}/
└── binance_spot/
    └── {symbol}/                    ← lowercase, e.g. btcusdt
        ├── 2026-04-25.mpack         ← orderbook snapshots
        ├── 2026-04-25-trades.mpack  ← trade events
        ├── 2026-04-25.mpack.zst         ← compressed after daily rotation
        └── 2026-04-25-trades.mpack.zst
```

Example for `btcusdt` and `ethusdt`:

```
data/binance_spot/btcusdt/2026-04-25.mpack
data/binance_spot/btcusdt/2026-04-25-trades.mpack
data/binance_spot/ethusdt/2026-04-25.mpack
data/binance_spot/ethusdt/2026-04-25-trades.mpack
```

The futures-channel `binance` exchange (`fstream.binance.com`) keeps the standard layout with no trade file. Spot is a separate `ExchangeName::BinanceSpot` and produces both files.

### Polymarket

Polymarket markets rotate on fixed windows (e.g. 5mins and 15mins). Each window produces four files — orderbook snapshots and last-trade records, each split by token side (Up / Down). The layout uses the **base slug** (no timestamp), the UTC date of the window start, and the window time range as the filename:

```
{base_path}/
└── {base_slug}/                   ← e.g. btc-updown-5m
    └── {YYYY-MM-DD}/              ← UTC date of window start
        ├── {HH:MM}-{HH:MM}-up.mpack
        ├── {HH:MM}-{HH:MM}-down.mpack
        ├── {HH:MM}-{HH:MM}-up-trades.mpack
        ├── {HH:MM}-{HH:MM}-down-trades.mpack
        ├── {HH:MM}-{HH:MM}-up.mpack.zst         ← compressed after window close
        ├── {HH:MM}-{HH:MM}-down.mpack.zst
        ├── {HH:MM}-{HH:MM}-up-trades.mpack.zst
        └── {HH:MM}-{HH:MM}-down-trades.mpack.zst
```

Example for a 5-minute BTC market:

```
data/polymarket/btc-updown-5m/2026-04-06/
    06:10-06:15-up.mpack
    06:10-06:15-down.mpack
    06:10-06:15-up-trades.mpack
    06:10-06:15-down-trades.mpack
    06:15-06:20-up.mpack
    06:15-06:20-down.mpack
    06:15-06:20-up-trades.mpack
    06:15-06:20-down-trades.mpack
    ...
```

Key layout rules:

- **Base slug only** — the timestamp suffix (e.g. `-1775456100`) is never part of the directory name. All windows for the same market land under the same slug folder.
- **UTC date** — the date directory reflects the window start time in UTC, not local time.
- **Window filename** — times are in `HH:MM` UTC. Snapshot files end in `-up.mpack` / `-down.mpack`; trade files end in `-up-trades.mpack` / `-down-trades.mpack`.
- **Token side** — Up and Down token data are always written to separate files. Trade records are routed to the correct side file by matching the `asset_id` from the wire event to the resolved token list.
- **Compression** — if `zstd_level > 0`, after the window closes the `.mpack` is replaced by `.mpack.zst`.

---

## Trade Record Schema

Trade files (`*-trades.mpack`) use a different schema from snapshot files. Each record is one matched maker-taker event:

| Field | Type | Description |
| -------------- | --------- | ----------------------------------------------- |
| `ts_ns` | i64 | UTC timestamp in nanoseconds since Unix epoch |
| `price` | f64 | Trade price |
| `size` | f64 | Trade size |
| `side` | String | Taker direction: `"BUY"` or `"SELL"` |
| `fee_rate_bps` | f64 | Fee rate in basis points (`0.0` for public feeds without fees) |
| `trade_id` | i64? | Exchange-assigned trade id — omitted for feeds that don't carry one |

`exchange` and `symbol` are not stored — they are encoded in the file path.

**Per-exchange notes:**

- **Polymarket** (`last_trade_price` WS channel): `price` is in USDC 0–1 (implied probability), `size` is in USDC, `fee_rate_bps` is set, `trade_id` is **absent** (the Polymarket feed does not include one).
- **Binance Spot** (`@trade` stream): `price` is the trade price, `size` is the base-asset quantity. `fee_rate_bps` is `0.0` (public feed has no fee info). `trade_id` is **present** and carries Binance's exchange-assigned trade id (`t` field). `side` is the taker side, derived from the wire `m` flag (buyer-is-maker `→ "SELL"`, otherwise `"BUY"`).

---

## Configuration

### Standard exchanges

All storage options live under `storage:` in `configs/server.yaml`:

```yaml
storage:
  enabled: true
  base_path: "./data"     # root directory for all data files
  depth: 20               # number of bid/ask levels stored per snapshot
  flush_interval: 1000    # ms between BufWriter flushes to disk
  rotation: "daily"       # "daily" | "none"
  zstd_level: 0           # 0 = disabled; 1–22 = zstd compression level
```

### Polymarket

Options live under `storage:` in `configs/polymarket.yaml`:

```yaml
storage:
  depth: 8                # orderbook levels to record per side
  base_path: "./data"
  flush_interval: 500     # ms between BufWriter flushes
  zstd_level: 3           # 0 = disabled; 1–22 = zstd compression level
```

### Rotation policy

| Value     | Behaviour                                                                                                                           |
| --------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `"daily"` | A new file is opened at each UTC midnight. The previous day's file is closed and optionally compressed.                             |
| `"none"`  | A single file per symbol is written for the lifetime of the process. Useful for short tests or when rotation is handled externally. |

### Compression level

After a daily rotation (or window close for Polymarket) the closed file is compressed in the background using zstd. The original `.mpack` is deleted once compression succeeds, leaving only the `.mpack.zst`.

| `zstd_level` | Behaviour                                                                        |
| ------------ | -------------------------------------------------------------------------------- |
| `0`          | Compression disabled. Files stay as `.mpack`.                                    |
| `1`          | Fastest compression, largest output (~35–40% of raw).                            |
| `3`          | Good balance of speed and ratio (recommended default).                           |
| `10–15`      | High compression, noticeably slower.                                             |
| `19–22`      | Maximum compression (`--ultra`), very slow. Only useful for archiving cold data. |

Rule of thumb: use `3` for live servers, `15` for nightly archival jobs.

---

## Price-to-Beat Archive (Polymarket / Kalshi)

Two binaries archive each resolved window's `priceToBeat` (start-of-window
oracle price) and `finalPrice` (end-of-window oracle price) to daily **CSV**
files:

- **`price_to_beat_fetcher`** — long-running daemon. Reads
  `configs/example.yaml`, drives one task per enabled market, **auto-backfills
  on startup** from the latest CSV row (or `--seed-from` if the archive is
  empty), then polls every interval boundary forever.
- **`price_to_beat_backfill`** — one-shot. Walks a single market over a
  user-specified `--from` / `--to` range. Useful for filling gaps in a single
  market without restarting the daemon.

Both share the same CSV format and dedup behaviour — they're safe to run
side-by-side, and the daemon's auto-backfill subsumes most ad-hoc backfill use
cases.

### Why CSV (not mpack)

- One row per 15-min (or 5-min) window → tens of rows per day, not millions.
- Designed for ad-hoc analysis: open in Excel, `pandas.read_csv`, `awk`, etc.
- Append-only and idempotent (dedup by `window_start`) — safe to re-run.

### File Layout

```
{archive_dir}/
└── {exchange}/                     ← `polymarket` or `kalshi`
    └── {base_slug_or_series}/      ← e.g. btc-updown-15m, btc-updown-5m, KXBTC15M
        ├── 2026-04-29.csv
        ├── 2026-04-30.csv
        └── ...
```

Examples:

```
data/price_to_beat/polymarket/btc-updown-15m/2026-04-29.csv
data/price_to_beat/polymarket/btc-updown-5m/2026-04-29.csv
data/price_to_beat/polymarket/eth-updown-15m/2026-04-29.csv
data/price_to_beat/kalshi/KXBTC15M/2026-04-29.csv
```

The date directory is the UTC date of the **window start**, not when the row
was written. A row written at 00:05 UTC for a 23:45 UTC window the previous
day lands in yesterday's file.

### Schema

| Column          | Type   | Description                                                                  |
| --------------- | ------ | ---------------------------------------------------------------------------- |
| `ts_recorded`   | i64    | Unix seconds when the row was written                                        |
| `exchange`      | string | `polymarket` or `kalshi`                                                     |
| `base_slug`     | string | Polymarket: base slug (e.g. `btc-updown-15m`). Kalshi: series (e.g. `KXBTC15M`) |
| `full_slug`     | string | Polymarket: full slug with window-start suffix. Kalshi: market ticker        |
| `window_start`  | i64    | Unix seconds at which the window opened (the dedup key)                      |
| `window_end`    | i64    | Unix seconds at which the window closed                                      |
| `price_to_beat` | f64    | Polymarket: `priceToBeat` (Chainlink price at window open). Kalshi: `floor_strike` (for `greater_or_equal` markets) or `cap_strike` (for `less_or_equal`) |
| `final_price`   | f64    | Polymarket: `finalPrice` (Chainlink price at close). Kalshi: `expiration_value` — the 60-second average of CF Benchmarks BRTI snapshotted at the window close |
| `direction`     | string | `"up"` / `"down"` outcome. Polymarket: derived from `final_price` vs `price_to_beat`. Kalshi: derived from the `result` field (`"yes"` → `"up"`, `"no"` → `"down"`), equivalent to the price comparison |

Header is written once per file at first append.

### Example

```
ts_recorded,exchange,base_slug,full_slug,window_start,window_end,price_to_beat,final_price,direction
1777479718,polymarket,btc-updown-15m,btc-updown-15m-1777467600,1777467600,1777468500,77126.6645582677,77170.16958187832,up
1777479718,polymarket,btc-updown-15m,btc-updown-15m-1777468500,1777468500,1777469400,77170.16958187832,76912.11387837428,down
1777479719,polymarket,btc-updown-15m,btc-updown-15m-1777469400,1777469400,1777470300,76912.11387837428,76549.62177890803,down
1777479719,polymarket,btc-updown-15m,btc-updown-15m-1777470300,1777470300,1777471200,76549.62177890803,76768.27,up
```

### Why the One-Hour Lookback

**Polymarket**: `priceToBeat` is **not** populated when a window first opens —
Polymarket resolves the field via the [Chainlink BTC/USD data stream](https://data.chain.link/streams/btc-usd)
only after the start tick has been observed and confirmed. Likewise,
`finalPrice` only appears after the close tick has been observed.

**Kalshi**: the strike (`floor_strike` / `cap_strike`) is fixed at market
creation, but `expiration_value` (the 60-second BRTI average at close) and
the `result` field (`"yes"` / `"no"`) only land once the market reaches
`status: "finalized"` — typically a few seconds after the close timestamp.

Both binaries target windows that **closed at least 1 hour ago** by default
(`--lookback-secs 3600` for the fetcher; pick `--to` accordingly for the
backfill). By that point all the fields above are reliably present.

For KX*15M markets the `floor_strike` of one window is the `expiration_value`
of the *previous* window — i.e. each market's strike is the previous window's
close. This means the CSV self-checks: `row[i].price_to_beat ==
row[i-1].final_price` for consecutive resolved windows.

### Live Daemon: `price_to_beat_fetcher`

```bash
cargo run --release --bin price_to_beat_fetcher -- \
    --config configs/example.yaml \
    --lookback-secs 3600 \
    --seed-from 2026-04-10T00:00:00Z \
    --archive-dir ./data/price_to_beat
```

Behaviour:

1. **Startup** — for each enabled market, scans the existing archive and
   resumes from `latest_window_start + interval`. If the market has no
   history on disk, it instead seeds from `--seed-from` (default
   `2026-04-10T00:00:00Z`). It then walks forward window-by-window up to
   `now - lookback_secs`, fetching any missing rows.
2. **Live loop** — sleeps until the next interval boundary, then fetches the
   window that closed `lookback_secs` ago. The CSV is the source of truth;
   if a row is already present (e.g. backfill caught up), the tick is a no-op.
3. **Per-market task** — every enabled Polymarket slug and Kalshi series in
   the config gets its own tokio task, so a slow upstream on one market
   doesn't block another.

The daemon is idempotent: restarting picks up exactly where it left off, and
two instances pointed at the same archive will simply skip each other's writes
(rows already on disk are matched by `window_start`).

### One-Shot Backfill: `price_to_beat_backfill`

For ad-hoc gap-filling on a single market over a fixed range:

```bash
# Polymarket 15-min market
cargo run --release --bin price_to_beat_backfill -- polymarket \
    --base-slug btc-updown-15m \
    --interval-secs 900 \
    --from 2026-04-25T00:00:00Z \
    --to 2026-04-29T00:00:00Z \
    --archive-dir ./data/price_to_beat \
    --http-timeout-secs 8

# Polymarket 5-min market — same shape, different cadence
cargo run --release --bin price_to_beat_backfill -- polymarket \
    --base-slug btc-updown-5m --interval-secs 300 \
    --from 2026-04-25T00:00:00Z

# Kalshi 15-min series
cargo run --release --bin price_to_beat_backfill -- kalshi \
    --series KXBTC15M --interval-secs 900 \
    --from 2026-04-25T00:00:00Z
```

Defaults:

- `--to` is *now*.
- `--archive-dir` is `./data/price_to_beat`.
- `--interval-secs` is `900` (15 min).
- Inter-request spacing is a compile-time constant (`REQUEST_SPACING_MS = 100`)
  to avoid hammering the upstream API. Edit the source if you need to tune it.

The tool reads every existing CSV under
`{archive_dir}/{exchange}/{base_slug_or_series}/` first and skips any
`window_start` already on disk, so re-running with overlapping `--from`/`--to`
is a no-op.

