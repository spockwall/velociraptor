# Storage — MessagePack Snapshot Files

Append-only binary files written by the recorder crate. Length-prefixed msgpack records (`u32 LE length` + `msgpack map`). One record = one orderbook snapshot or one trade.

## Snapshot record

| Field | Type | Description |
|---|---|---|
| `sequence` | u64 | Monotonically increasing |
| `ts_ns` | i64 | UTC nanoseconds |
| `bids` | `[[f64, f64]]` | `[price, qty]`, best first |
| `asks` | `[[f64, f64]]` | best first |

`N` levels = `storage.depth`. Exchange/symbol encoded in the file path.

## Trade record (`*-trades.mpack`)

`ts_ns`, `price`, `size`, `side` (`"BUY"`/`"SELL"`), `fee_rate_bps`, optional `trade_id`.

## File layout

| Source | Path |
|---|---|
| Standard (Binance futures, OKX, Hyperliquid) | `{base_path}/{exchange}/{SYMBOL}/{YYYY-MM-DD}.mpack` |
| Binance Spot (snapshots + trades) | `{base_path}/binance_spot/{symbol}/{YYYY-MM-DD}.mpack` + `…-trades.mpack` |
| Polymarket (rolling windows) | `{base_path}/{base_slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-{up\|down}[-trades].mpack` |

After daily rotation (or window close), files are zstd-compressed in the background (`zstd_level > 0`) and `.mpack` is replaced by `.mpack.zst`.

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

Full record schemas, per-exchange notes (Polymarket no `trade_id`, Binance Spot `m` flag), all archive details, why the 1-hour oracle lookback, Python reader recipes (`read_mpack`, `read_mpack_zst`, Polymarket helper script, CSV with pandas) are in the project skill **`velociraptor-storage`** at `.claude/skills/velociraptor-storage/SKILL.md`.
