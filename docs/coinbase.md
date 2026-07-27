# Coinbase Exchange market data

The Coinbase connector collects public spot order books and executions from
`wss://ws-feed.exchange.coinbase.com`. It requires no API credentials.

## Channels

| Data | Channel | Behavior |
|---|---|---|
| Order book | `level2_batch` | Initial full snapshot, then absolute price-level updates batched every 50 ms |
| Public trades | `matches` | One message per execution; Coinbase reports the maker side |

The unbatched `level2` channel currently requires authentication. The connector
uses `level2_batch`, which Coinbase documents as the public alternative with the
same snapshot/update schema. A zero-size level is removed from the book.

Coinbase's `match.side` is the resting maker side. Stored trades use taker
direction, so maker `sell` becomes `BUY` and maker `buy` becomes `SELL`. The
first `last_match` message after each subscription is a historical replay and is
ignored to avoid duplicate records after reconnects.

Coinbase warns that `matches` messages can be dropped. The current connector
does not backfill gaps from REST, so the trade archive is a best-effort public
stream rather than a guaranteed complete tape. The `level2_batch` order book is
the delivery-guaranteed Level 2 channel.

## Configuration

Coinbase is a static exchange in the unified configuration:

```yaml
coinbase:
  enabled: true
  symbols:
    - "BTC-USD"
    - "ETH-USD"
```

Both `orderbook_server` and `orderbook_recorder` use this section. Run the
standalone archive recorder with:

```bash
cargo run -p orderbook --bin orderbook_recorder --release -- \
  --config configs/dev/recorder.yaml
```

## Storage

The generic `StorageWriter` uses the same daily snapshot/trade naming as
Binance Spot:

```text
{storage.base_path}/coinbase/BTC-USD/2026-07-22.mpack
{storage.base_path}/coinbase/BTC-USD/2026-07-22-trades.mpack
```

Snapshot records contain `sequence`, `ex_timestamp`, `recv_timestamp`, `bids`,
and `asks`. Coinbase snapshots have `ex_timestamp = 0` because the snapshot
message has no venue timestamp; subsequent updates use the RFC3339 `time` from
Coinbase. Trade records contain `ex_timestamp`, `recv_timestamp`, `price`,
`size`, taker `side`, `fee_rate_bps = 0.0`, and the numeric `trade_id`.

With `storage.rotation: "daily"`, dates are UTC. When `zstd_level > 0`, completed
files become `.mpack.zst`.

## Runtime topics

`orderbook_server` publishes the normal static-exchange topics:

```text
coinbase:BTC-USD
coinbase:BTC-USD:last_trade
```

When Redis is enabled, the latest book and capped history use the corresponding
`ob:coinbase:*`, `bba:coinbase:*`, `snapshots:coinbase:*`, and
`trades:coinbase:*` keys.

## Official references

- [WebSocket overview](https://docs.cdp.coinbase.com/exchange/websocket-feed/overview)
- [WebSocket channels](https://docs.cdp.coinbase.com/exchange/websocket-feed/channels)
