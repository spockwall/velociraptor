# Kalshi — Auth, Ticker, Orderbook Mechanism

CFTC-regulated US prediction market. Auth required even for market data.

## Rolling 15-min crypto markets

| Series | Description |
|---|---|
| `KXBTC15M` | BTC up/down next 15 min |
| `KXETH15M` | ETH up/down next 15 min |

Each market asks: is final price ≥ a target set at window open? Two contracts (YES, NO) trade on the same book. Windows are 15 min, aligned to UTC `:00/:15/:30/:45`.

## Ticker format

```
KXBTC15M - 26APR160700 - 00
│          │              └─ strike suffix (per-window, assigned by Kalshi)
│          └─ close time in ET (DST-aware): YYMONDDHHMM
└─ series ticker
```

First two segments = **event ticker** (deterministic from clock). Full **market ticker** with strike suffix must be resolved via REST before subscribing:

```
GET https://api.elections.kalshi.com/trade-api/v2/markets?event_ticker={event}
→ markets[0].ticker
```

Code entry points: `orderbook::exchanges::kalshi::build_event_ticker(series, close_utc)` and `resolve_market_ticker(event_ticker)`.

## Authentication

Three signed headers on the `GET /trade-api/ws/v2` upgrade:

| Header | Value |
|---|---|
| `KALSHI-ACCESS-KEY` | API key UUID |
| `KALSHI-ACCESS-TIMESTAMP` | Unix ms (string) |
| `KALSHI-ACCESS-SIGNATURE` | base64( RSA-PSS-SHA256(`timestamp + "GET" + "/trade-api/ws/v2"`) ) |

Credentials file (`.gitignore`d):

```yaml
kalshi:
  key_id: "<uuid>"
  private_key: |
    -----BEGIN PRIVATE KEY-----
    <pem>
    -----END PRIVATE KEY-----
  ws_url: "wss://api.elections.kalshi.com/trade-api/ws/v2"
```

## Subscription message

Channels and tickers as **separate arrays** (concatenating fails silently):

```json
{
  "id": 1, "cmd": "subscribe",
  "params": {
    "channels": ["orderbook_delta"],
    "market_tickers": ["KXBTC15M-26APR160700-00", "KXETH15M-26APR160700-00"]
  }
}
```

Server-initiated ping `{"type":"ping"}`; client pongs automatically.

## Orderbook mechanism (complement)

YES + NO sum to $1.00. A NO-bid at $0.56 = YES-ask at $0.44. The parser merges both bid-ladders into one two-sided book in YES space:

| Wire | Book side | Book price |
|---|---|---|
| `yes_dollars_fp` | Bid | `price` |
| `no_dollars_fp` | Ask | `1 - price` |
| delta `side:yes` | Bid | `price_dollars` |
| delta `side:no` | Ask | `1 - price_dollars` |

`delta_fp > 0` → `Update`; `≤ 0` → `Delete` (qty=0). Visualiser renders the same book twice — UP panel as-is, DOWN panel mirrored — spread is identical in both.

## Configuration

```yaml
kalshi:
  market:
    - { enable: true, series: "KXBTC15M", interval_secs: 900 }
    - { enable: true, series: "KXETH15M", interval_secs: 900 }
```

```bash
cargo run --example kalshi_orderbook --release -- \
    --config configs/kalshi.yaml --credentials credentials/kalshi.yaml
```

## Auto-rotation scheduler (per series)

1. Compute current window close from wall clock.
2. `build_event_ticker` → REST `resolve_market_ticker`. On REST failure, wait 5s + retry.
3. If different from currently-connected, spawn new `MarketTask`, drop old.
4. Sleep until `early_start_secs` before close (fresh `Utc::now()` sample).
5. Resolve next window's ticker and pre-start (brief parallel overlap).
6. Sleep remainder until current expires.
7. Drop old; promote pre-started; repeat.

## Redis integration

Mirrors Polymarket — scheduler + per-window engine + watchdog. One label per market (no UP/DOWN pairing — Kalshi gives one combined two-sided book).

`kalshi:label:{ticker}` hash:

```
series        = "KXBTC15M"
ticker        = "KXBTC15M-26APR160700-00"
window_start  = "1776681900"     # Unix s, aligned to :00/:15/:30/:45
interval_secs = "900"
```

`window_start = (now + 30) / interval * interval` — the `+30` jumps past pre-start overlap so the value resolves to the new window's boundary. Backend `GET /api/kalshi/markets` reads `kalshi:label:index`, lazy-expires stale entries, sorts by `(series, window_start, ticker)`.

## Deep reference

Full architecture (auth signing, scheduler details, eviction loops, watchdog cancellation semantics, frontend hook, wire-format comparison table) is in the project skill **`velociraptor-kalshi`** at `.claude/skills/velociraptor-kalshi/SKILL.md`.

Related skills:

- `velociraptor-wire-formats` — raw `orderbook_snapshot` / `orderbook_delta` payloads
- `velociraptor-storage` — price-to-beat CSV archive
