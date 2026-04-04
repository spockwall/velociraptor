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

| Field          | Type            | Description                                      |
|----------------|-----------------|--------------------------------------------------|
| `exchange`     | string          | Exchange name, e.g. `"binance"`                  |
| `symbol`       | string          | Symbol, e.g. `"BTCUSDT"`                         |
| `sequence`     | u64             | Monotonically increasing update counter          |
| `ts_ns`        | i64             | UTC timestamp in nanoseconds since Unix epoch    |
| `best_bid_px`  | f64 or null     | Best bid price                                   |
| `best_bid_qty` | f64 or null     | Best bid quantity                                |
| `best_ask_px`  | f64 or null     | Best ask price                                   |
| `best_ask_qty` | f64 or null     | Best ask quantity                                |
| `spread`       | f64 or null     | `best_ask_px - best_bid_px`                      |
| `mid`          | f64 or null     | `(best_bid_px + best_ask_px) / 2`                |
| `wmid`         | f64             | Quantity-weighted mid price                      |
| `bids`         | `[[f64, f64]]`  | Top-N bids as `[price, qty]`, best first         |
| `asks`         | `[[f64, f64]]`  | Top-N asks as `[price, qty]`, best first         |

`N` is controlled by the `depth` setting in `configs/server.toml`. Null values appear when the book is empty on one side.

---

## File Layout

Files are organised by exchange, symbol, and date:

```
{base_path}/
└── {exchange}/
    └── {SYMBOL}/
        ├── 2026-04-01.mpack
        ├── 2026-04-02.mpack
        ├── 2026-04-02.mpack.zst   ← compressed after rotation (if enabled)
        └── 2026-04-03.mpack
```

Examples:

```
data/binance/BTCUSDT/2026-04-03.mpack
data/hyperliquid/BTC/2026-04-03.mpack
data/okx/BTC-USDT/2026-04-03.mpack
data/polymarket/75467129.../2026-04-03.mpack
```

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

| Value     | Behaviour |
|-----------|-----------|
| `"daily"` | A new file is opened at each UTC midnight. The previous day's file is closed and optionally compressed. |
| `"none"`  | A single file per symbol is written for the lifetime of the process. Useful for short tests or when rotation is handled externally. |

### Compression level

After a daily rotation the closed file is compressed in the background using zstd. The original `.mpack` is deleted once compression succeeds, leaving only the `.mpack.zst`.

| `zstd_level` | Behaviour |
|--------------|-----------|
| `0`          | Compression disabled. Files stay as `.mpack`. |
| `1`          | Fastest compression, largest output (~35–40% of raw). |
| `3`          | Good balance of speed and ratio (recommended default). |
| `10–15`      | High compression, noticeably slower. |
| `19–22`      | Maximum compression (`--ultra`), very slow. Only useful for archiving cold data. |

Rule of thumb: use `3` for live servers, `15` for nightly archival jobs.

---

## Reading the Data

### Python — quick script

```bash
pip install msgpack pandas
python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
python scripts/read_mpack.py data/binance/BTCUSDT/   # loads all dates in directory
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

## Estimating Disk Usage

At 100ms update rate with 20 depth levels, each record is approximately 800–1200 bytes after msgpack serialisation.

| Symbols | Update rate | Raw (daily) | zstd-3 (daily) |
|---------|-------------|-------------|----------------|
| 1       | 100ms       | ~700 MB     | ~140 MB        |
| 4       | 100ms       | ~2.8 GB     | ~560 MB        |
| 10      | 100ms       | ~7 GB       | ~1.4 GB        |

Orderbooks are highly repetitive between consecutive snapshots, so zstd achieves ~5:1 compression ratios on typical data.
