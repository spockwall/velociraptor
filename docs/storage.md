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

### Binance Spot (`binance_spot`)

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

### Polymarket (`polymarket_recorder`)

Polymarket markets rotate on fixed windows (e.g. every 5 minutes). Each window produces four files — orderbook snapshots and last-trade records, each split by token side (Up / Down). The layout uses the **base slug** (no timestamp), the UTC date of the window start, and the window time range as the filename:

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

## Reading the Data

### Python — quick script

```bash
pip install msgpack pandas zstandard

python scripts/read_mpack.py data/binance/BTCUSDT/          # all files in directory
python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
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
```

### Reading compressed files

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

### Working with the depth levels

The `bids` and `asks` columns contain lists of `[price, qty]` pairs:

```python
import numpy as np

row = df.iloc[0]

bids = np.array(row["bids"])   # shape (N, 2)
asks = np.array(row["asks"])

bid_prices, bid_qtys = bids[:, 0], bids[:, 1]
ask_prices, ask_qtys = asks[:, 0], asks[:, 1]

# Derived metrics
best_bid, best_ask = bid_prices[0], ask_prices[0]
mid    = (best_bid + best_ask) / 2
spread = best_ask - best_bid
wmid   = (best_ask * bid_qtys[0] + best_bid * ask_qtys[0]) / (bid_qtys[0] + ask_qtys[0])
```

---

## Reading Binance Spot Data

Binance Spot uses the standard `{base_path}/binance_spot/{symbol}/{date}.mpack` layout for orderbook snapshots, plus a sibling `{date}-trades.mpack` for raw trades.

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

# Orderbook snapshots
ob = read_mpack("data/binance_spot/btcusdt/2026-04-25.mpack")
print(ob[["ts", "sequence"]].head())

# Trades — one row per matched fill
trades = read_mpack("data/binance_spot/btcusdt/2026-04-25-trades.mpack")
print(trades[["ts", "price", "size", "side", "trade_id"]].head())
```

`trade_id` is an `int64` from Binance's `t` field — useful for de-duplication when joining multiple data sources or replaying.

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

The script adds metadata columns derived from the filename, and computes `mid`, `spread`, and `wmid` from the stored `bids`/`asks`:

| Column   | Example       | Description                          |
| -------- | ------------- | ------------------------------------ |
| `window` | `06:15-06:20` | Window time range (UTC)              |
| `side`   | `up` / `down` | Token direction                      |
| `mid`    | `0.445`       | `(best_bid + best_ask) / 2`          |
| `spread` | `0.01`        | `best_ask - best_bid`                |
| `wmid`   | `0.447`       | Quantity-weighted mid price          |

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

### Reading trade files

Trade files use the same length-prefixed msgpack format as snapshot files but have a flat schema — no `bids`/`asks` arrays.

```python
import struct, msgpack, pandas as pd

def read_trades(path):
    records = []
    with open(path, "rb") as f:
        while header := f.read(4):
            (n,) = struct.unpack("<I", header)
            records.append(msgpack.unpackb(f.read(n), raw=False))
    df = pd.DataFrame(records)
    df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df

# Up-side trades for one window
up_trades = read_trades("data/polymarket/btc-updown-5m/2026-04-06/06:15-06:20-up-trades.mpack")
print(up_trades[["ts", "price", "size", "side", "fee_rate_bps"]].head())
```

For compressed files pass the decompressed stream to the same unpacking loop (see `read_mpack_zst` above).

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

Each record stores `sequence` (8 bytes), `ts_ns` (8 bytes), and `depth × 2` f64 values for bids and asks. At depth=8 that is ~280 bytes raw per record before msgpack overhead.

**Standard exchanges:**

| Symbols | Update rate | Raw (daily) | zstd-3 (daily) |
| ------- | ----------- | ----------- | -------------- |
| 1       | 100ms       | ~700 MB     | ~140 MB        |
| 4       | 100ms       | ~2.8 GB     | ~560 MB        |

**Polymarket (per slug, Up + Down combined, depth=8):**

Polymarket update rates vary with market activity. During volatile periods updates can arrive hundreds of times per second.

| Slugs | Avg update rate | Raw per day  | zstd-3 per day |
| ----- | --------------- | ------------ | -------------- |
| 1     | high-freq       | ~10–30 GB    | ~2–6 GB        |
| 4     | high-freq       | ~40–120 GB   | ~8–24 GB       |

Orderbooks are highly repetitive between consecutive snapshots, so zstd achieves ~5:1 compression ratios on typical data.
