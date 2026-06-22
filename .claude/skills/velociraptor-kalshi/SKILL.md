---
name: velociraptor-kalshi
description: Kalshi integration — RSA-PSS auth on WS upgrade, ticker format and resolution, YES/NO complement orderbook mechanism, scheduler, Redis key schema, frontend hook. Use when working on anything Kalshi-specific.
---

# Velociraptor — Kalshi

CFTC-regulated US prediction market. Auth REQUIRED even for market data.

For wire format see `velociraptor-wire-formats`. For storage see `velociraptor-storage`.

## Rolling 15-minute crypto markets

| Series | Description |
|---|---|
| `KXBTC15M` | BTC up/down in next 15 minutes |
| `KXETH15M` | ETH up/down in next 15 minutes |

Each market asks: *is final price at window close ≥ a target set at window open?* Two contracts trade on the **same book**: YES (≥ target) and NO (< target). Both are bid-only on the wire; this project synthesises a single two-sided YES-perspective book.

Windows are 15min, aligned to UTC `:00/:15/:30/:45`. A new market opens every 15 minutes with a fresh ticker.

## Ticker format

```
KXBTC15M - 26APR160700 - 00
│          │              │
│          │              └─ strike suffix (per-window, assigned by Kalshi)
│          └─ close time in ET: YYMONDDHHMM
│             (e.g. 26 Apr 16, 07:00 EDT = 11:00 UTC)
└─ series ticker
```

First two segments = **event ticker** (`KXBTC15M-26APR160700`). Full **market ticker** (with strike suffix) is what `subscribe` and REST order endpoints expect.

### Why the strike suffix isn't constant

Each window has one binary market whose strike is set from the prevailing index price at window open, rounded to a Kalshi grid. Suffix can be `00`, `15`, `30`, etc. **Not derivable from the clock** — must be resolved via REST before subscribing.

### Two-step construction

1. **Event ticker** — deterministic from wall clock. Embed segment is window **close** in US Eastern (EDT/EST, DST-aware via `chrono-tz`):

   ```
   now        = Utc::now()
   win_close  = round_up_to_next_15min_boundary(now)
   event      = "{series}-{format_et(win_close)}"   // KXBTC15M-26APR160700
   ```

   | UTC close | ET | Segment |
   |---|---|---|
   | 2026-04-16 11:00 UTC | 07:00 EDT | `26APR160700` |
   | 2026-01-13 10:30 UTC | 05:30 EST | `26JAN130530` |

2. **Market ticker** — via REST:

   ```
   GET https://api.elections.kalshi.com/trade-api/v2/markets?event_ticker={event}
   ```

   `markets[0].ticker` (e.g. `KXBTC15M-26APR160700-00`) is the string for `subscribe`.

### Code entry points

- `orderbook::exchanges::kalshi::build_event_ticker(series, close_utc) -> String`
- `orderbook::exchanges::kalshi::resolve_market_ticker(event_ticker) -> Result<String, _>`

The scheduler in `orderbook/examples/kalshi_orderbook.rs` calls these once per rotation (current window on first tick, next `EARLY_START_SECS` before close).

## Authentication

Three headers signed on every `GET /trade-api/ws/v2` upgrade:

| Header | Value |
|---|---|
| `KALSHI-ACCESS-KEY` | API key UUID from Kalshi dashboard |
| `KALSHI-ACCESS-TIMESTAMP` | Current Unix time in ms (string) |
| `KALSHI-ACCESS-SIGNATURE` | base64( RSA-PSS-SHA256(`timestamp + "GET" + "/trade-api/ws/v2"`) ) |

`KalshiConnection` signs automatically. Without valid headers → HTTP 401, no data.

Get key pair: https://kalshi.com/account/profile/api-keys — generate RSA, register public key, store private locally.

### Credentials file (`credentials/kalshi.yaml`)

```yaml
kalshi:
  key_id: "<your-key-id-uuid>"
  private_key: |
    -----BEGIN PRIVATE KEY-----
    <your-rsa-private-key-pem>
    -----END PRIVATE KEY-----
  ws_url: "wss://api.elections.kalshi.com/trade-api/ws/v2"
```

`.gitignore`d. Pass with `--credentials credentials/kalshi.yaml`.

## Subscription message

Channels and tickers are **separate arrays**. Concatenating (`"orderbook_delta:TICKER"`) returns a subscribed ack but no data.

```json
{
  "id": 1, "cmd": "subscribe",
  "params": {
    "channels": ["orderbook_delta"],
    "market_tickers": ["KXBTC15M-26APR160700-00", "KXETH15M-26APR160700-00"]
  }
}
```

Ping: server-initiated `{"type":"ping"}` every ~10s; the parser detects via `is_ping()` and the connection sends a WS pong control frame automatically.

## Orderbook mechanism (complement relation)

Binary contract identity: `price(YES) + price(NO) = 1.00` at resolution and (by no-arbitrage) in the book. So a NO-bid at $0.56 is economically a YES-ask at $0.44.

Wire → book mapping (always from YES perspective):

| Wire | Book side | Book price |
|---|---|---|
| `yes_dollars_fp[price, qty]` | Bid | `price` |
| `no_dollars_fp[price, qty]` | Ask | `1 - price` |
| `delta side:yes` | Bid | `price_dollars` |
| `delta side:no` | Ask | `1 - price_dollars` |

Sizes copied through (dollars, not contracts). Negative deltas → `OrderbookAction::Delete` with `qty=0`.

After parsing:
- **Best bid** = max `yes_dollars_fp.price`
- **Best ask** = `1 − max(no_dollars_fp.price)`
- **Spread** = tight spread ⇒ YES+NO together cost near $1.00
- **Mid** = market-implied probability of YES

### Visualiser two-panel display

Single internal book, two panels side-by-side:
- **UP (YES):** book as-is.
- **DOWN (NO):** mirror — `no_bid(p) = 1 − yes_ask(p)`, `no_ask(p) = 1 − yes_bid(p)`, `no_mid = 1 − yes_mid`. Spread is identical in both panels.

## Visualiser config (`configs/dev/kalshi.yaml`)

```yaml
server:  { render_interval: 300 }
storage: { depth: 10 }
kalshi:
  market:
    - { enable: true, series: "KXBTC15M", interval_secs: 900 }
    - { enable: true, series: "KXETH15M", interval_secs: 900 }
```

Run:

```bash
cargo run --example kalshi_orderbook --release -- \
    --config configs/dev/kalshi.yaml --credentials credentials/kalshi.yaml

# CLI flags alternative
cargo run --example kalshi_orderbook --release -- \
    --series KXBTC15M --series KXETH15M --depth 8 \
    --credentials credentials/kalshi.yaml
```

## Auto-rotation scheduler

Per series, independent loop:

1. Compute current window close from wall clock.
2. `build_event_ticker` → REST `resolve_market_ticker`. On REST failure, wait 5s and retry.
3. If different from currently-connected, spawn new `MarketTask`, drop old.
4. Sleep until `early_start_secs` before close (sample `Utc::now()` fresh — WebSocket handshake from step 3 is accounted for).
5. Resolve next window's ticker and pre-start its connection (both run briefly in parallel).
6. Sleep remainder until current expires (fresh `Utc::now()` sample).
7. Drop old; promote pre-started; repeat.

## Redis integration

Mirrors Polymarket architecture — scheduler owns lifecycle, per-window engine writes Redis via an inline forward hook, watchdog handles label cleanup at shutdown.

### Key schema

Storage keys are keyed by **`series`**, not `ticker`. Overwritten by every snapshot, including across ticker rollovers — subscribers see a continuous series.

| Key | Type | Lifetime | Written by |
|---|---|---|---|
| `ob:kalshi:{series}` | string (msgpack) | overwritten each tick | per-window forward hook |
| `bba:kalshi:{series}` | string (msgpack) | overwritten each tick | per-window forward hook |
| `snapshots:kalshi:{series}` | list | LPUSH+LTRIM to `snapshot_cap` | per-window forward hook |
| `trades:kalshi:{series}` | list | LPUSH+LTRIM to `trade_cap` | per-window forward hook |
| `kalshi:label:{ticker}` | hash | one per live ticker | window setup |
| `kalshi:label:index` | set of `ticker` | mirrors live labels | window setup |
| `kalshi:series:{series}:tickers` | set of `ticker` | per series | window setup |

The current window's `ticker` is carried inside the msgpack payload as `OrderbookSnapshot.full_slug: Option<String>` so subscribers detect rollover from the payload — no resubscribe needed.

**Do not** call `attach_redis(&mut engine, ...)` on per-window engines. It would key by `snap.symbol` (= ticker) and re-introduce ticker-keyed storage. It's attached only to the main engine for static exchanges.

No UP/DOWN pairing in Redis — Kalshi has one combined two-sided book per market, so each window contributes exactly **one** label.

Label hash:
```
series         = "KXBTC15M"
ticker         = "KXBTC15M-26APR160700-00"
window_start   = "1776681900"     # Unix s — UTC :00/:15/:30/:45
interval_secs  = "900"
```

> **`window_start` derivation.** Kalshi windows are clock-aligned and always 15 min, so `window_start = (now + 30) / interval * interval`. The `+30` jumps past the pre-start overlap so the value resolves to the new window's boundary. No ticker parsing needed (unlike Polymarket).

### Window setup (`spawn_kalshi_window`, `zmq_server/src/setup.rs`)

1. Build connection config with resolved ticker, WS URL, signed creds.
2. Evict expired prior-window **labels** for this series (same logic as Polymarket). Does NOT touch ob/bba/snapshots/trades — those are keyed by `series` and reused across rollovers.
3. Register new ticker under `kalshi:series:{series}:tickers`.
4. Write `kalshi:label:{ticker}`, add to `kalshi:label:index`.
5. Spin up per-window `StreamEngine`. Register a forward hook on `OrderbookSnapshot` / `LastTradePrice` that stamps `full_slug = Some(ticker.clone())`, `bus.publish(StreamEvent::RollingSnapshot { base_slug: series, snap })`, and writes the same payload to Redis under `ob:kalshi:{series}` etc.
6. Watchdog task polling `SystemControl.is_shutdown()` removes only the LABEL this window owned. Storage keys stay live for the next window.

### Backend read path (`GET /api/kalshi/markets`)

1. Read `kalshi:label:index`.
2. Load label hash; empty → orphan, SREM and skip.
3. Lazy expiry if `window_start + interval_secs <= now` → `DEL kalshi:label:{ticker}` + `SREM kalshi:label:index`. Does NOT touch ob/bba/snapshots/trades.
4. Sort by `(series, window_start, ticker)`.

Response:

```json
[{
  "ticker": "KXBTC15M-26APR160700-00",
  "series": "KXBTC15M",
  "window_start": 1776681900,
  "interval_secs": 900,
  "title": "KXBTC15M-26APR160700-00"
}]
```

Steady-state: `COUNT label == # enabled series`, transient +1/series during pre-start.

## Frontend

`/kalshi` (`frontend/src/pages/Kalshi.tsx`) polls `/api/kalshi/markets` every 5s, renders one panel per live series with best bid / spread / best ask + depth-bar of top-12 levels per side. Panels are keyed by `series` (not `ticker`), so the React component instance — and its `usePolling` state — **survives ticker rollover**. The panel's title comes from `snap.full_slug` (the live ticker), which flips when the new window's first snapshot arrives. Backend fetcher uses `api.orderbook("kalshi", market.series)` — i.e. the series, since that's the stable Redis key.
