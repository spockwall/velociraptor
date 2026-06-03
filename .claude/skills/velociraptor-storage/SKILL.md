---
name: velociraptor-storage
description: MessagePack snapshot file format used by the recorder crate — wire format, record schemas, per-exchange file layouts, trade-file schema, config knobs, Python readers (mpack + zstd). Use when reading, writing, or replaying recorded data.
---

# Velociraptor — Storage Format

Append-only binary files written by the recorder crate. MessagePack chosen for compactness, schema flexibility (add fields without breaking old readers), and language-agnostic libs.

Each record is a **msgpack map** (string keys → values), not an array — field names are preserved so files are readable without a schema.

## Wire format

Flat sequence of length-prefixed records:

```
┌──────────┬─────────────────────────────────────┐
│ 4 bytes  │  N bytes                            │
│ u32 LE   │  msgpack map                        │
│ (length) │  { "sequence": 1, "ts_ns": ... }    │
└──────────┴─────────────────────────────────────┘
```

Records are written sequentially, no padding, no index. Always read front-to-back.

## Snapshot record schema

| Field | Type | Description |
|---|---|---|
| `sequence` | u64 | Monotonically increasing update counter |
| `ts_ns` | i64 | UTC timestamp in nanoseconds |
| `bids` | `[[f64, f64]]` | Top-N bids `[price, qty]`, best first |
| `asks` | `[[f64, f64]]` | Top-N asks |

`N` is `storage.depth`. `exchange` / `symbol` / `spread` / `mid` / `wmid` are not stored — exchange/symbol are encoded in the path; derived metrics are computed at read time.

## Trade record schema (`*-trades.mpack`)

| Field | Type | Description |
|---|---|---|
| `ts_ns` | i64 | UTC ns |
| `price` | f64 | Trade price |
| `size` | f64 | Trade size |
| `side` | str | Taker direction `"BUY"`/`"SELL"` |
| `fee_rate_bps` | f64 | Fee bps (0.0 for public feeds) |
| `trade_id` | i64? | Exchange-assigned id (omitted for feeds without it) |

Per-exchange notes:
- **Polymarket** (`last_trade_price`): `price` in USDC 0–1 (implied probability), `size` in USDC, `fee_rate_bps` set, **no** `trade_id`.
- **Binance Spot** (`@trade`): trade price + base-asset qty, `fee_rate_bps=0.0`, `trade_id` present (`t` field). `side` derived from `m` flag (buyer-is-maker → `"SELL"`).

## File layout

### Standard exchanges (Binance futures, OKX, Hyperliquid)

```
{base_path}/{exchange}/{SYMBOL}/2026-04-03.mpack
                                 2026-04-02.mpack.zst   ← compressed after rotation
```

### Binance Spot (snapshots + raw trades)

```
{base_path}/binance_spot/{symbol}/2026-04-25.mpack          ← orderbook
                                  2026-04-25-trades.mpack   ← trades
                                  2026-04-25.mpack.zst
                                  2026-04-25-trades.mpack.zst
```

The futures `binance` exchange uses the standard layout with no trade file. Spot is a separate `ExchangeName::BinanceSpot`.

### User events (CSV, not mpack)

The `recorder` crate's `StorageWriter` (used by `orderbook_recorder` + `orderbook_server`) writes **three** streams. Snapshots and trades are mpack as above; the **account-wide user-event stream is CSV** — it's low volume (a handful of fills / order-updates) so it stays human-readable like the price-to-beat archive, and is never zstd-compressed:

```
{base_path}/events/{YYYY-MM-DD}.csv   ← fills + order_updates, `type` column discriminates
```

Columns: `ts_ns,type,exchange,symbol,side,px,qty,filled,status,fee,taker_oid,client_oid,exchange_oid,trade_id,maker_orders`. `maker_orders` is a JSON string in one cell.

The recorder crate splits the two formats by module: `recorder/src/mpack_writer.rs` holds `StorageWriter` (the mpack snapshot/trade streams), and `recorder/src/csv.rs` holds `CsvArchive` — the shared append-only CSV helper the user-event stream and both fetchers use.

### Polymarket (rolling windows)

Four files per window: orderbook snapshots × 2 sides, trades × 2 sides.

```
{base_path}/{base_slug}/{YYYY-MM-DD}/        ← UTC date of window start
    {HH:MM}-{HH:MM}-up.mpack
    {HH:MM}-{HH:MM}-down.mpack
    {HH:MM}-{HH:MM}-up-trades.mpack
    {HH:MM}-{HH:MM}-down-trades.mpack
    *.mpack.zst                              ← after window close (if zstd_level > 0)
```

Rules:
- **Base slug only** — no timestamp suffix in directory name.
- **UTC** date.
- Up and Down written to separate files; trade records routed by `asset_id`.

## Config

```yaml
storage:
  enabled: true
  base_path: "./data"
  depth: 20
  flush_interval: 1000    # ms between BufWriter flushes
  rotation: "daily"       # "daily" | "none"
  zstd_level: 0           # 0 = disabled, 1–22 = zstd level (recommend 3 live, 15 archive)
```

Rotation: `"daily"` rolls at UTC midnight; previous day's file is optionally zstd-compressed in the background and the `.mpack` is removed once compression succeeds.

## Price-to-Beat archive (Polymarket + Kalshi)

Daily **CSV** files (not mpack — tens of rows/day, designed for pandas/Excel):

```
{archive_dir}/{exchange}/{base_slug_or_series}/{YYYY-MM-DD}.csv
```

Examples:
```
data/price_to_beat/polymarket/btc-updown-15m/2026-04-29.csv
data/price_to_beat/kalshi/KXBTC15M/2026-04-29.csv
```

The date directory is the UTC date of the **window start**, not write time.

Schema:

| Column | Type | Description |
|---|---|---|
| `ts_recorded` | i64 | Unix seconds when written |
| `exchange` | str | `polymarket` \| `kalshi` |
| `base_slug` | str | Polymarket base slug or Kalshi series |
| `full_slug` | str | Polymarket full slug or Kalshi market ticker |
| `window_start` | i64 | Unix seconds — dedup key |
| `window_end` | i64 | Unix seconds |
| `price_to_beat` | f64 | Polymarket: `priceToBeat`. Kalshi: `floor_strike` / `cap_strike` |
| `final_price` | f64 | Polymarket: `finalPrice`. Kalshi: `expiration_value` |
| `direction` | str | `"up"` / `"down"` |

**Why 1-hour lookback by default.** Polymarket's `priceToBeat` / `finalPrice` are not populated until Chainlink reports; Kalshi's `expiration_value` / `result` land when status is `"finalized"`. Both binaries default to `--lookback-secs 3600`. For Kalshi rolling 15M, `row[i].price_to_beat == row[i-1].final_price` (self-check).

Binaries:
- `price_to_beat_fetcher` — long-running daemon; auto-backfills on startup from latest CSV row (or `--seed-from`), then polls every interval boundary.
- `price_to_beat_backfill` — one-shot, walks a single market over `--from`/`--to`.

Both share CSV format and dedup by `window_start` — safe to run side-by-side. The file mechanics (append-with-header, read-all, daily path) come from `recorder::CsvArchive`; the binary only defines the row struct + partition (`&[exchange, base_slug]`).

### Adding a new CSV dataset

`recorder::CsvArchive` is the one place to go through for any low-volume tabular data:

```rust
use recorder::CsvArchive;
#[derive(serde::Serialize, serde::Deserialize)]
struct Row { ts: i64, /* … */ }

let archive = CsvArchive::new("data/my_dataset");          // root
let path = archive.daily_path(&["polymarket", slug], day); // {root}/polymarket/{slug}/{date}.csv
archive.append(&path, &row)?;                              // header written on create
let rows: Vec<Row> = archive.read_dir(&["polymarket", slug]); // every row across all days
```

`append` creates parent dirs + writes the header only on a new/empty file (append-safe on restart); `read_dir` skips malformed rows. No rotation/compression — CSV is for human-readable, low-volume data.

## Asset-ID archive (Polymarket only)

Records `market_id`, `yes_asset_id`, `no_asset_id` per window. Kalshi skipped (no yes/no token pair). Layout: `data/asset_ids/polymarket/{base_slug}/{YYYY-MM-DD}.csv`. Auto-backfills on startup.

```
ts_recorded, base_slug, full_slug, window_start, window_end, market_id, yes_asset_id, no_asset_id
```

`market_id` is integer Polymarket market id (stringified). `yes_asset_id` / `no_asset_id` are `clobTokenIds[0]` / `[1]`. Each rolling window has a fresh market id and token pair.

## Python readers

### mpack

```python
import struct, msgpack, pandas as pd

def read_mpack(path):
    records = []
    with open(path, "rb") as f:
        while header := f.read(4):
            (n,) = struct.unpack("<I", header)
            records.append(msgpack.unpackb(f.read(n), raw=False))
    df = pd.DataFrame(records)
    df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df

df = read_mpack("data/binance/BTCUSDT/2026-04-03.mpack")
```

### mpack.zst

```python
import zstandard, io, struct, msgpack, pandas as pd

def read_mpack_zst(path):
    dctx = zstandard.ZstdDecompressor()
    records = []
    with open(path, "rb") as fh, dctx.stream_reader(fh) as reader:
        buf = io.BufferedReader(reader)
        while header := buf.read(4):
            (n,) = struct.unpack("<I", header)
            records.append(msgpack.unpackb(buf.read(n), raw=False))
    df = pd.DataFrame(records)
    df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df
```

### Depth levels and derived metrics

```python
import numpy as np
row  = df.iloc[0]
bids = np.array(row["bids"])        # shape (N, 2)
asks = np.array(row["asks"])

best_bid, best_ask = bids[0, 0], asks[0, 0]
mid     = (best_bid + best_ask) / 2
spread  = best_ask - best_bid
wmid    = (best_ask * bids[0, 1] + best_bid * asks[0, 1]) / (bids[0, 1] + asks[0, 1])
```

### Polymarket helper script

```bash
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/06:15-06:20-up.mpack
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/           # all dates/sides
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --side up
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --summary
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --merge
```

Adds `window`, `side`, `mid`, `spread`, `wmid` columns derived from filename + stored bids/asks.

### Price-to-beat CSV

```python
import pandas as pd
df = pd.read_csv("data/price_to_beat/polymarket/btc-updown-15m/2026-04-29.csv")
for c in ("ts_recorded","window_start","window_end"):
    df[c] = pd.to_datetime(df[c], unit="s", utc=True)

print(df["direction"].value_counts(normalize=True))
df["return"] = df["final_price"] - df["price_to_beat"]
```

Load all dates: `pd.concat([pd.read_csv(p) for p in sorted(Path(d).glob("*.csv"))])`.
