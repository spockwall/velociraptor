# Storage — MessagePack Snapshot Files

The recorder crate persists every orderbook snapshot to disk as an append-only binary file using the MessagePack format.

---

## Why MessagePack

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
│  │ (length) │  { "exchange": "binance", ... }     │ │
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
| `exchange` | string         | Exchange name, e.g. `"binance"`               |
| `symbol`   | string         | Symbol, e.g. `"BTCUSDT"`                      |
| `sequence` | u64            | Monotonically increasing update counter       |
| `ts_ns`    | i64            | UTC timestamp in nanoseconds since Unix epoch |
| `spread`   | f64 or null    | `best_ask_px - best_bid_px`                   |
| `mid`      | f64 or null    | `(best_bid_px + best_ask_px) / 2`             |
| `wmid`     | f64            | Quantity-weighted mid price                   |
| `bids`     | `[[f64, f64]]` | Top-N bids as `[price, qty]`, best first      |
| `asks`     | `[[f64, f64]]` | Top-N asks as `[price, qty]`, best first      |


`N` is controlled by the `depth` setting in `configs/server.toml`. Null values appear when the book is empty on one side.

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

### Polymarket (`polymarket_recorder`)

Polymarket markets rotate on fixed windows (e.g. every 5 minutes). Each window produces two files — one for the Up token and one for the Down token. The layout uses the **base slug** (no timestamp), the UTC date of the window start, and the window time range as the filename:

```
{base_path}/
└── polymarket/
    └── {base_slug}/              ← e.g. btc-updown-5m
        └── {YYYY-MM-DD}/         ← UTC date of window start
            ├── {HH:MM}-{HH:MM}-up.mpack
            ├── {HH:MM}-{HH:MM}-down.mpack
            ├── {HH:MM}-{HH:MM}-up.mpack.zst    ← compressed after window close
            └── {HH:MM}-{HH:MM}-down.mpack.zst
```

Example for a 5-minute BTC market:

```
data/polymarket/btc-updown-5m/2026-04-06/
    06:10-06:15-up.mpack
    06:10-06:15-down.mpack
    06:15-06:20-up.mpack
    06:15-06:20-down.mpack
    ...
```

Key layout rules:

- **Base slug only** — the timestamp suffix (e.g. `-1775456100`) is never part of the directory name. All windows for the same market land under the same slug folder.
- **UTC date** — the date directory reflects the window start time in UTC, not local time.
- **Window filename** — times are in `HH:MM` UTC, e.g. `06:15-06:20-up.mpack`.
- **Compression** — if `zstd_level > 0`, after the window closes the `.mpack` is replaced by `.mpack.zst`.

---

## Configuration

All storage options live under `[storage]` in `configs/server.toml`:

```toml
[storage]
enabled        = true
base_path      = "./data"   # root directory for all data files
depth          = 20         # number of bid/ask levels stored per snapshot
flush_interval = 1000       # ms between BufWriter flushes to disk
rotation       = "daily"    # "daily" | "none"
zstd_level     = 0          # 0 = disabled; 1–22 = zstd compression level
```

### Rotation policy


| Value     | Behaviour                                                                                                                           |
| --------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `"daily"` | A new file is opened at each UTC midnight. The previous day's file is closed and optionally compressed.                             |
| `"none"`  | A single file per symbol is written for the lifetime of the process. Useful for short tests or when rotation is handled externally. |


### Compression level

After a daily rotation the closed file is compressed in the background using zstd. The original `.mpack` is deleted once compression succeeds, leaving only the `.mpack.zst`.


| `zstd_level` | Behaviour                                                                        |
| ------------ | -------------------------------------------------------------------------------- |
| `0`          | Compression disabled. Files stay as `.mpack`.                                    |
| `1`          | Fastest compression, largest output (~35–40% of raw).                            |
| `3`          | Good balance of speed and ratio (recommended default).                           |
| `10–15`      | High compression, noticeably slower.                                             |
| `19–22`      | Maximum compression (`--ultra`), very slow. Only useful for archiving cold data. |


Rule of thumb: use `3` for live servers, `15` for nightly archival jobs.

---

## Reading the Data

### Python — quick script

```bash
pip in
python scripts/read_mpack.py data/binance/BTCUSDT/   # loads all dastall msgpack pandas
python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpacktes in directory
```

### Python — in your own code

```python
import struct, msgpack, pandas as pd
from pathlib import Path

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
print(df[["ts", "best_bid_px", "best_ask_px", "spread", "wmid"]].head())
```

### Reading compressed files

Decompress first, then read:

```python
import zstandard, io, struct, msgpack, pandas as pd

def read_mpack_zst(path):
    dctx = zstandard.ZstdDecompressor()
    records = []
    with open(path, "rb") as fh:
        with dctx.stream_reader(fh) as reader:
            buf = io.BufferedReader(reader)
            while header := buf.read(4):
                (n,) = struct.unpack("<I", header)
                records.append(msgpack.unpackb(buf.read(n), raw=False))
    df = pd.DataFrame(records)
    df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df

df = read_mpack_zst("data/binance/BTCUSDT/2026-04-02.mpack.zst")
```

Install: `pip install zstandard`

### Working with the depth levels

The `bids` and `asks` columns contain lists of `[price, qty]` pairs:

```python
import numpy as np

row = df.iloc[0]

bids = np.array(row["bids"])   # shape (N, 2)
asks = np.array(row["asks"])

bid_prices, bid_qtys = bids[:, 0], bids[:, 1]
ask_prices, ask_qtys = asks[:, 0], asks[:, 1]

# Total bid liquidity in top 5 levels
print(bid_qtys[:5].sum())
```

---

---

## Reading Polymarket Data

Polymarket files have a different directory structure than standard exchanges, so there is a dedicated script.

### Quick script

```bash
pip install msgpack pandas zstandard

# One window side
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/06:15-06:20-up.mpack

# All windows for a date (both sides)
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/

# All dates for a slug
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/

# Only the Up side
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --side up

# Per-window summary (record count, mean mid, mean spread)
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --summary

# Merge up and down into aligned rows
python scripts/read_polymarket.py data/polymarket/btc-updown-5m/2026-04-06/ --merge
```

The script adds two metadata columns derived from the filename:


| Column   | Example       | Description             |
| -------- | ------------- | ----------------------- |
| `window` | `06:15-06:20` | Window time range (UTC) |
| `side`   | `up` / `down` | Token direction         |


### In your own code

```python
from scripts.read_polymarket import load, load_side, merge_sides, summary

# Load all windows for a date
df = load("data/polymarket/btc-updown-5m/2026-04-06/")
print(df[["ts", "window", "side", "mid", "spread"]].head())

# Load only the Up side
up = load_side("data/polymarket/btc-updown-5m/2026-04-06/", "up")

# Align Up and Down snapshots by nearest timestamp (±500ms)
merged = merge_sides("data/polymarket/btc-updown-5m/2026-04-06/")
print(merged[["ts", "window_up", "mid_up", "mid_down", "sum_mid"]].head())

# Per-window summary table
print(summary("data/polymarket/btc-updown-5m/2026-04-06/"))
```

### Interpreting prices

Polymarket tokens are priced in USDC between 0.0 and 1.0 (= implied probability):

```python
df = load("data/polymarket/btc-updown-5m/2026-04-06/")

# Implied probability of Up winning each window
up = df[df["side"] == "up"].copy()
up["implied_prob"] = up["mid"]          # mid ≈ market probability
up["spread_bps"]   = up["spread"] * 10_000

# Check that Up + Down ≈ 1 (minus fees) across matched snapshots
m = merge_sides("data/polymarket/btc-updown-5m/2026-04-06/")
print(m["sum_mid"].describe())          # should be near 0.96–1.00
```

---

## Estimating Disk Usage

At 100ms update rate with 20 depth levels, each record is approximately 800–1200 bytes after msgpack serialisation.

**Standard exchanges:**


| Symbols | Update rate | Raw (daily) | zstd-3 (daily) |
| ------- | ----------- | ----------- | -------------- |
| 1       | 100ms       | ~700 MB     | ~140 MB        |
| 4       | 100ms       | ~2.8 GB     | ~560 MB        |
| 10      | 100ms       | ~7 GB       | ~1.4 GB        |


**Polymarket (per slug, Up + Down combined):**

Polymarket update rates vary with market activity. At rest the book may not update for seconds; during volatile periods updates arrive every 100–200ms.


| Slugs | Avg update rate | Raw per window (5m) | Raw per day  | zstd-3 per day |
| ----- | --------------- | ------------------- | ------------ | -------------- |
| 1     | ~500ms avg      | ~0.5–2 MB           | ~150–600 MB  | ~30–120 MB     |
| 4     | ~500ms avg      | ~2–8 MB             | ~600 MB–2 GB | ~120–400 MB    |


Orderbooks are highly repetitive between consecutive snapshots, so zstd achieves ~5:1 compression ratios on typical data.