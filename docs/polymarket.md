# Polymarket — Market Mechanism and Data Recording

This document explains how Polymarket prediction markets work, how this project connects to them, and how the `polymarket_recorder` binary stores data to disk.

---

## Market Mechanism

### What is Polymarket?

Polymarket is a decentralised prediction market where participants trade YES/NO outcome tokens on future events. Each token is priced between $0 and $1, representing the market's implied probability that the outcome resolves YES (or NO).

**Example:** "Will BTC close above $100k on 2026-04-15?"
- YES token price of $0.72 = 72% implied probability
- NO token price of $0.28 = 28% implied probability
- At resolution: the correct token pays out $1; the other pays $0

### Rolling Up/Down Markets

This project focuses on a specific market type: **rolling Up/Down** (also called UpDown or Pump/Dump) markets. These markets ask whether an asset's price will go up or down over a fixed window, then reset and repeat.

**Example slug:** `btc-updown-5m`

| Property | Description |
|----------|-------------|
| **Window size** | 5 minutes (300 seconds), 15 minutes (900 seconds), etc. |
| **Outcome** | Up token = BTC price goes up by the end of the window. Down token = price goes down. |
| **Settlement** | At window end, one token settles to $1 and the other to $0. |
| **Continuity** | A new market immediately opens for the next window with fresh token IDs. |

### Token IDs

Each side of each window is identified by a unique 256-bit integer token ID (a Polymarket `clobTokenId`). Token IDs change every window:

```
btc-updown-5m-1775315700  →  Up token:   7546712...
                          →  Down token: 3842963...
btc-updown-5m-1775316000  →  Up token:   1234567...  (new window, new IDs)
                          →  Down token: 9876543...
```

Token IDs must be resolved from the **Gamma REST API** before subscribing. This project handles that automatically.

### Slug Format

Window slugs are named `{base_slug}-{unix_timestamp}` where the timestamp is the window **start time** (floor of `now / interval * interval`):

```
btc-updown-5m-1775315700
               └─ Unix timestamp of window start (seconds)
```

Static markets (no rolling window) use the slug directly with no timestamp suffix:

```
will-btc-reach-100k-in-2025
```

### Order Book

Polymarket uses a Central Limit Order Book (CLOB) for each token. The orderbook reflects the current bids and asks for that token priced in USDC. This project streams the orderbook via the Polymarket WebSocket API:

- **Snapshot message** (`book`): full book state delivered on connect
- **Incremental update** (`price_change`): diffs applied to the local book

---

## Token Resolution

At startup (and before each window rotation), the project resolves token IDs by calling the Gamma REST API:

```
GET https://gamma-api.polymarket.com/markets/slug/{slug}
```

The response body contains a `clobTokenIds` field with two entries:
- Index 0 = Up/Yes token ID
- Index 1 = Down/No token ID

This happens synchronously before the WebSocket connection is opened. If the market is not open yet (404), the connection is skipped and retried on the next rotation.

---

## Configuration

Both the visualiser and recorder are configured identically. Create a YAML file:

```yaml
# configs/polymarket.yaml

polymarket:
  markets:
    - enabled: true
      slug: "btc-updown-5m"
      interval_secs: 300    # window size in seconds; 0 = static market

    - enabled: true
      slug: "eth-updown-5m"
      interval_secs: 300

    # Static market (slug is used directly, no timestamp appended):
    # - enabled: true
    #   slug: "will-btc-reach-100k-in-2025"
    #   interval_secs: 0
```

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | bool | Skip this entry if false |
| `slug` | string | Base slug from the Polymarket URL |
| `interval_secs` | u64 | Window size in seconds. `0` = static market |

---

## Running the Visualiser

The `polymarket_orderbook` example shows a live terminal UI with the orderbook for each configured market:

```bash
# From config file
cargo run --example polymarket_orderbook --release -- --config configs/polymarket.yaml

# From CLI flags
cargo run --example polymarket_orderbook --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --slug eth-updown-5m --interval-secs 300 \
    --depth 8
```

The terminal displays one panel per market side (Up/Down), updating at `render_interval` milliseconds. The panel title shows the fully-timestamped slug so you know which window is live.

---

## Running the Recorder

The `polymarket_recorder` binary does everything the visualiser does, plus writes every orderbook snapshot to disk immediately (not at the render rate):

```bash
# From config file
cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml

# From CLI flags
cargo run --bin polymarket_recorder --release -- \
    --slug btc-updown-5m --interval-secs 300 \
    --base-path ./data \
    --depth 10 \
    --zstd-level 3
```

### Recorder Config

Add these fields to the YAML config file (alongside the `polymarket:` section):

```yaml
server:
  render_interval: 300    # ms between terminal redraws (independent of write rate)

storage:
  depth: 10               # orderbook levels to record per side
  base_path: "./data"
  flush_interval: 1000    # ms between BufWriter flushes
  zstd_level: 3           # 0 = disabled, 1–22 = zstd level
```

---

## File Layout

The recorder writes one file per window per side:

```
{base_path}/polymarket/{base_slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-up.mpack
                                              /{HH:MM}-{HH:MM}-down.mpack
```

**Example** for a 5-minute BTC market window from 09:55–10:00 UTC:

```
data/
└── polymarket/
    └── btc-updown-5m/
        └── 2026-04-05/
            ├── 09:55-10:00-up.mpack
            ├── 09:55-10:00-down.mpack
            ├── 10:00-10:05-up.mpack
            └── 10:00-10:05-down.mpack
```

If `zstd_level > 0`, after each window closes the file is compressed in the background and the original is removed:

```
09:55-10:00-up.mpack.zst     ← replaces 09:55-10:00-up.mpack
09:55-10:00-down.mpack.zst
```

### Window Lifecycle

1. Window opens → recorder resolves token IDs from Gamma API, opens `.mpack` files for Up and Down
2. Every snapshot event → record written immediately (at WebSocket update rate, typically 100–500ms)
3. Every second → BufWriter flushed to disk
4. `EARLY_START_SECS=10` before window end → next window's task is pre-started (overlap period)
5. Window expires → old task stopped, writers flushed and closed, zstd compression spawned in background
6. New window's task becomes the sole writer

---

## Record Format

Each record uses the same length-prefixed MessagePack format as the main orderbook server (see `docs/storage.md`):

```
┌──────────┬──────────────────────────────────────────────┐
│ 4 bytes  │  N bytes                                     │
│ u32 LE   │  msgpack map { "exchange": ..., "bids": ... } │
│ (length) │                                              │
└──────────┴──────────────────────────────────────────────┘
```

| Field | Type | Description |
|-------|------|-------------|
| `exchange` | string | Always `"polymarket"` |
| `symbol` | string | Token ID (256-bit integer as string) |
| `sequence` | u64 | Monotonically increasing update counter |
| `ts_ns` | i64 | UTC timestamp in nanoseconds since Unix epoch |
| `spread` | f64 or null | `best_ask - best_bid` |
| `mid` | f64 or null | `(best_bid + best_ask) / 2` |
| `wmid` | f64 | Quantity-weighted mid price |
| `bids` | `[[f64, f64]]` | Top-N bids as `[price, qty]`, best first |
| `asks` | `[[f64, f64]]` | Top-N asks as `[price, qty]`, best first |

Prices are in USDC (0.00–1.00 range for prediction market tokens).

---

## Reading the Data in Python

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

# Load one window side
df = read_mpack("data/polymarket/btc-updown-5m/2026-04-05/09:55-10:00-up.mpack")
print(df[["ts", "mid", "spread", "wmid"]].head())

# Load all files for a slug
import glob
files = sorted(glob.glob("data/polymarket/btc-updown-5m/**/*.mpack", recursive=True))
df = pd.concat([read_mpack(f) for f in files], ignore_index=True)
```

For compressed files, see the zstd reading example in `docs/storage.md`.

### Interpreting Prices

Prices for Up/Down tokens are in USDC per token (range 0.0–1.0):

```python
df["implied_prob_up"] = df["mid"]          # mid price ≈ implied probability of Up
df["spread_bps"] = df["spread"] * 10_000  # spread in basis points

# Reconstruct Up/Down token prices for each timestamp
up_df   = read_mpack("09:55-10:00-up.mpack")
down_df = read_mpack("09:55-10:00-down.mpack")

merged = up_df[["ts", "mid"]].rename(columns={"mid": "up_mid"}).merge(
    down_df[["ts", "mid"]].rename(columns={"mid": "down_mid"}),
    on="ts", how="outer"
)
merged["sum"] = merged["up_mid"] + merged["down_mid"]  # should be ≈ 1.0 minus fees
```

---

## Disk Usage Estimates

At a typical Polymarket update rate of one update per 100–500ms with 10 depth levels:

| Markets | Update rate | Raw per window | zstd-3 per window |
|---------|-------------|----------------|-------------------|
| 1 slug (Up+Down) | 200ms avg | ~2–5 MB | ~0.5–1 MB |
| 4 slugs | 200ms avg | ~8–20 MB | ~2–4 MB |

Daily totals depend heavily on market activity. Quiet windows generate far fewer updates.
