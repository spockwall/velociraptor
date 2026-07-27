# Storage — MessagePack Snapshot Files

Append-only binary files written by the recorder crate. Length-prefixed msgpack records (`u32 LE length` + `msgpack map`). One record = one orderbook snapshot, trade, or CF Benchmarks value update.

## Snapshot record

| Field | Type | Description |
|---|---|---|
| `sequence` | u64 | Monotonically increasing |
| `ts_ns` | i64 | UTC nanoseconds |
| `bids` | `[[f64, f64]]` | `[price, qty]`, best first |
| `asks` | `[[f64, f64]]` | best first |

`N` levels = `storage.depth`. Exchange/symbol encoded in the file path.

## Trade record (`*-trades.mpack`)

`ts_ns`, `price`, `size`, `side` (`"BUY"`/`"SELL"`), `fee_rate_bps`, optional `trade_id` (integer or venue UUID string).

## File layout

| Source | Path |
|---|---|
| Standard (Binance futures, OKX, Hyperliquid) | `{base_path}/{exchange}/{SYMBOL}/{YYYY-MM-DD}.mpack` |
| Binance Spot (snapshots + trades) | `{base_path}/binance_spot/{symbol}/{YYYY-MM-DD}.mpack` + `…-trades.mpack` |
| Coinbase Exchange (snapshots + trades) | `{base_path}/coinbase/{product_id}/{YYYY-MM-DD}.mpack` + `…-trades.mpack` |
| Polymarket (rolling windows) | `{base_path}/{base_slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-{up\|down}[-trades].mpack` |
| Kalshi (rolling windows) | `{base_path}/{series}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}[-trades].mpack` |
| Kalshi CF Benchmarks | `{base_path}/cfbenchmarks/{index_id}/{YYYY-MM-DD}.mpack` |

After daily rotation (or window close), files are zstd-compressed in the background (`zstd_level > 0`) and `.mpack` is replaced by `.mpack.zst`.

Coinbase uses the public `level2_batch` and `matches` channels. Initial book
snapshots have `ex_timestamp = 0`; updates and trades use Coinbase's RFC3339
event time. Stored trade `side` is taker direction (the inverse of Coinbase's
maker-side field), and the numeric `trade_id` is retained. See
[Coinbase Exchange market data](coinbase.md) for channel and completeness notes.

## Kalshi CF Benchmarks

`kalshi_cfbenchmark_recorder` subscribes to Kalshi's authenticated
`cfbenchmarks_value` WebSocket channel. It records `BRTI` and `ETHUSD_RTI` by
default; repeat `--index` or pass comma-separated IDs to override the defaults.
Kalshi emits updates roughly once per second per index.

```bash
cargo run -p orderbook --bin kalshi_cfbenchmark_recorder -- \
  --config configs/dev/kalshi.yaml \
  --kalshi-credentials credentials/dev/kalshi.yaml
```

### Benchmark record

Exact benchmark and average values are stored as decimal strings to avoid
precision loss.

| Field | Type | Description |
|---|---|---|
| `sid` | u64 | WebSocket subscription ID |
| `seq` | u64 | Kalshi sequence number |
| `index_id` | str | CF Benchmarks index, such as `BRTI` or `ETHUSD_RTI` |
| `received_at` | i64 | Time Kalshi received the source frame, Unix milliseconds |
| `raw_data` | str | Original JSON string supplied in Kalshi's `data` field |
| `source_data` | map | Parsed upstream value: `type`, `id`, `time`, and decimal-string `value` |
| `avg_60s_data` | map | Trailing 60-second average, window size, and window boundaries |
| `last_60s_windowed_average_15min` | map? | Quarter-hour closing-window average; omitted outside the final minute before `:00`, `:15`, `:30`, or `:45` |
| `recv_timestamp` | i64 | Local recorder receive time, Unix nanoseconds |

Average maps contain `value`, `window_size`, `window_start_ts_ms`, and
`window_end_ts_exclusive`. The source map's `time` is the upstream Unix
millisecond timestamp.

### Benchmark file layout

With `storage.rotation: "daily"`, each index gets one UTC file per day:

```text
{storage.base_path}/cfbenchmarks/BRTI/2026-07-22.mpack
{storage.base_path}/cfbenchmarks/ETHUSD_RTI/2026-07-22.mpack
```

Completed days become `.mpack.zst` when `storage.zstd_level > 0`. With
`storage.rotation: "none"`, each index instead appends to `data.mpack`.

Read one file, one index directory, or the full benchmark tree with:

```bash
python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/
python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/BRTI/ --summary
python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/ --index ETHUSD_RTI
```

## Config

```yaml
storage:
  enabled: true
  base_path: "./data"
  depth: 20
  flush_interval: 1000    # ms
  rotation: "daily"       # "daily" | "none"
  zstd_level: 0           # 0 = off, 3 live, 15 archive
```

## Price-to-Beat archive (CSV)

`{archive_dir}/{exchange}/{base_slug_or_series}/{YYYY-MM-DD}.csv` with columns `ts_recorded, exchange, base_slug, full_slug, window_start, window_end, price_to_beat, final_price, direction`. Daemon `price_to_beat_fetcher` auto-backfills; `price_to_beat_backfill` is the one-shot variant. Default `--lookback-secs 3600` because the oracle reports late.

## Asset-ID archive (CSV, Polymarket only)

`data/asset_ids/polymarket/{base_slug}/{YYYY-MM-DD}.csv` with `market_id, yes_asset_id, no_asset_id` per window.

## Deep reference

Full record schemas, per-exchange notes (Polymarket no `trade_id`, Binance Spot `m` flag, Coinbase maker-side inversion), all archive details, why the 1-hour oracle lookback, Python reader recipes (`read_mpack`, `read_mpack_zst`, Polymarket helper script, CSV with pandas) are in the project skill **`velociraptor-storage`** at `.claude/skills/velociraptor-storage/SKILL.md`.
