# Kalshi — Market Mechanism and Orderbook Streaming

This document explains how Kalshi prediction markets work, how this project connects to them, and how the auto-rotating scheduler handles rolling 15-minute windows.

---

## What is Kalshi?

Kalshi is a regulated US prediction market exchange (CFTC-registered). Participants trade YES/NO binary contracts on future events. Each contract is priced between $0 and $1, representing the market's implied probability.

**Example:** "Will BTC price go up in the next 15 minutes?"
- YES price $0.68 = 68% implied probability of "up"
- NO price $0.32 = 32% implied probability of "down"
- At resolution: the correct side settles to $1; the other to $0

### Rolling 15-Minute Crypto Markets

This project targets Kalshi's **rolling 15-minute price direction markets** for BTC and ETH:

| Series | Description |
|--------|-------------|
| `KXBTC15M` | Bitcoin price up or down in the next 15 minutes |
| `KXETH15M` | Ethereum price up or down in the next 15 minutes |

Each market asks: *is the asset price at window close higher than a target price set at window open?* Every market has two contracts that trade on the same book:
- **YES** — buyers think the final price will be **at or above** the target
- **NO**  — buyers think the final price will be **below** the target

Both contracts are bid-only on the wire: Kalshi's book carries a list of YES bids and a separate list of NO bids (there are no ask ladders in the raw feed). This project converts the two into a single two-sided book from the YES contract's perspective — see [Orderbook mechanism](#orderbook-mechanism) below.

Windows are 15 minutes long and aligned to UTC clock boundaries (:00, :15, :30, :45). A new market opens every 15 minutes with a fresh ticker.

---

## Ticker Format

A Kalshi market ticker has three dash-separated segments:

```
KXBTC15M - 26APR160700 - 00
│          │              │
│          │              └─ strike suffix (per-window, assigned by Kalshi)
│          └─ close time in ET: YYMONDDHHММ
│             26 Apr 16, 07:00 AM EDT = 11:00 UTC
└─ series ticker
```

The first two segments form the **event ticker** (`KXBTC15M-26APR160700`). The full **market ticker** (with the strike suffix) is what the WebSocket `subscribe` and REST order endpoints expect.

### Why the strike suffix isn't a constant

Each 15-minute window has a single binary market whose target ("strike") price is set from the prevailing index price at window open and rounds to the nearest Kalshi-defined grid. The suffix encodes that strike for bookkeeping — it can be `00`, `15`, `30`, or whatever the round price maps to for this window. It is **not** derivable from the clock, so the client must resolve it from the REST API before subscribing.

### Two-step ticker construction

1. **Event ticker** — computed deterministically from the wall clock. The clock-embedded segment is the window **close** time converted to US Eastern Time (EDT/EST, DST-aware via `chrono-tz`):

   ```
   now = Utc::now()
   win_close = round_up_to_next_15min_boundary(now)   # UTC
   event = "{series}-{format_et(win_close)}"          # e.g. KXBTC15M-26APR160700
   ```

   | UTC close time       | Eastern time       | Segment          |
   |----------------------|--------------------|------------------|
   | 2026-04-16 11:00 UTC | 07:00 EDT (UTC-4)  | `26APR160700`    |
   | 2026-01-13 10:30 UTC | 05:30 EST (UTC-5)  | `26JAN130530`    |

2. **Market ticker** — resolved via the public REST endpoint before each subscription:

   ```
   GET https://api.elections.kalshi.com/trade-api/v2/markets?event_ticker={event}
   ```

   Response carries `markets[0].ticker` (e.g. `KXBTC15M-26APR160700-00`) — that's the string passed to `subscribe`.

### Code entry points

- `orderbook::exchanges::kalshi::build_event_ticker(series, close_utc) -> String`
- `orderbook::exchanges::kalshi::resolve_market_ticker(event_ticker) -> Result<String, _>`

The scheduler in `orderbook/examples/kalshi_orderbook.rs` calls these once per window rotation (current window on first tick, next window `EARLY_START_SECS` before close).

---

## Authentication

**Kalshi's WebSocket requires signed HTTP headers on the upgrade request.**

Three headers are added to every `GET /trade-api/ws/v2` upgrade:

| Header | Value |
|--------|-------|
| `KALSHI-ACCESS-KEY` | API key UUID from Kalshi dashboard |
| `KALSHI-ACCESS-TIMESTAMP` | Current Unix time in milliseconds (string) |
| `KALSHI-ACCESS-SIGNATURE` | base64( RSA-PSS-SHA256( `timestamp + "GET" + "/trade-api/ws/v2"` ) ) |

`KalshiConnection` signs the request automatically using `key_id` and `private_key` from the credentials file. Without valid headers the server returns `HTTP 401` and no data flows.

**Get a key pair:** https://kalshi.com/account/profile/api-keys — generate an RSA key pair, register the public key on Kalshi, store the private key locally.

Store credentials in `credentials/kalshi.yaml` (excluded from git via `.gitignore`):

```yaml
kalshi:
  key_id: "<your-key-id-uuid>"
  private_key: |
    -----BEGIN PRIVATE KEY-----
    <your-rsa-private-key-pem>
    -----END PRIVATE KEY-----
```

Copy `credentials/example.yaml` as a starting point. The file is separate from the display config (`configs/kalshi.yaml`) so secrets never end up in version control.

---

## WebSocket API

**Endpoint:** `wss://api.elections.kalshi.com/trade-api/ws/v2?apiKey=<key>`
**Subscription:** one message covering all tickers
**Ping:** server-initiated — Kalshi sends `{"type":"ping"}` every ~10s; client must respond with a pong (handled automatically by `KalshiMessageParser`)
**Symbol format:** full market ticker — `KXBTC15M-26APR130415-15`

### Subscription message

Channels and market tickers are passed as **separate** arrays. Do **not** concatenate them (`"orderbook_delta:TICKER"` is silently ignored by the server — you get a `subscribed` ack but no data).

```json
{
  "id": 1,
  "cmd": "subscribe",
  "params": {
    "channels": ["orderbook_delta"],
    "market_tickers": [
      "KXBTC15M-26APR160700-00",
      "KXETH15M-26APR160700-00"
    ]
  }
}
```

Multiple tickers can be included in one subscribe command.

### Subscription ack

```json
{ "type": "subscribed", "sid": 1, "seq": 1, "msg": {} }
```

The parser logs the confirmation and emits no `OrderbookMessage`.

### `orderbook_snapshot` — full book state

Sent immediately after subscribing. Replaces the entire orderbook.

```json
{
  "type": "orderbook_snapshot",
  "sid": 2,
  "seq": 2,
  "msg": {
    "market_ticker": "FED-23DEC-T3.00",
    "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
    "yes_dollars_fp": [
      ["0.0800", "300.00"],
      ["0.2200", "333.00"]
    ],
    "no_dollars_fp": [
      ["0.5400", "20.00"],
      ["0.5600", "146.00"]
    ]
  }
}
```

| Field | Description |
|-------|-------------|
| `market_ticker` | Used as the orderbook `symbol` |
| `yes_dollars_fp` | `[[price, size]]` — YES bids (people willing to pay `price` for YES) |
| `no_dollars_fp`  | `[[price, size]]` — NO bids (people willing to pay `price` for NO) |

Prices are string decimals in USD (0.00–1.00). Sizes are string-decimal dollar amounts.

**Action:** `OrderbookAction::Snapshot`. The parser converts the two bid-ladders into a single two-sided YES-perspective book — see [Orderbook mechanism](#orderbook-mechanism).

### `orderbook_delta` — incremental update

Sent on each order placement or cancellation. One message = one price level change.

```json
{
  "type": "orderbook_delta",
  "sid": 2,
  "seq": 3,
  "msg": {
    "market_ticker": "FED-23DEC-T3.00",
    "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
    "price_dollars": "0.960",
    "delta_fp": "-54.00",
    "side": "yes",
    "ts": "2022-11-22T20:44:01Z"
  }
}
```

| Field | Description |
|-------|-------------|
| `price_dollars` | Price level being updated (string decimal in USD) |
| `delta_fp` | **Signed change** in dollar size. Positive = level grew. Negative = level shrank or was removed. |
| `side` | `"yes"` → YES-bid change; `"no"` → NO-bid change (translated to a YES-ask at `1 - price`) |
| `ts` | RFC3339 timestamp (optional; falls back to `Utc::now()`) |

`delta_fp` is a **delta**, not an absolute size. The orderbook engine accumulates these on top of the snapshot.

**Action:** `delta_fp > 0` → `OrderbookAction::Update`; `delta_fp ≤ 0` → `OrderbookAction::Delete` (size set to 0, engine removes the level).

### Server ping

```json
{ "type": "ping", "id": 42 }
```

The parser detects this via `is_ping()` and the connection infrastructure sends a WebSocket pong control frame automatically. No data is emitted.

### Error message

```json
{ "type": "error", "msg": { "code": 401, "message": "Unauthorized" } }
```

Logged as an error. Most commonly caused by a missing or invalid API key.

---

## Orderbook mechanism

Kalshi's binary contracts (YES + NO) sum to $1.00 at resolution. That identity lets us reconstruct a traditional two-sided orderbook from the exchange's two bid-only ladders.

### The complement relation

For any binary market priced in dollars:

```
price(YES) + price(NO) = 1.00   (at resolution — and by no-arbitrage, in the book)
```

So a NO-bid at `$0.56` is economically identical to someone offering to **sell** YES at `$1.00 - $0.56 = $0.44`. That's a YES-ask at `$0.44`.

### From wire to book

The parser (`orderbook/src/exchanges/kalshi/msg_parser.rs`) emits every update into a single symbol (the market ticker) from the YES contract's perspective:

| Wire field / side | Book side | Book price |
|-------------------|-----------|-----------|
| `yes_dollars_fp[price, qty]`      | Bid | `price` (as-is)   |
| `no_dollars_fp[price, qty]`       | Ask | `1 - price`       |
| `orderbook_delta` with `side:yes` | Bid | `price_dollars`   |
| `orderbook_delta` with `side:no`  | Ask | `1 - price_dollars` |

Sizes are copied through unchanged (dollars, not contracts). Negative deltas become `OrderbookAction::Delete` with `qty=0` so the engine can drop or shrink the level regardless of its prior accumulated size.

After parsing, the `Orderbook` contains both sides in YES-price space:
- **Best bid** = highest YES buyer = `max(yes_dollars_fp.price)`
- **Best ask** = lowest (complemented) NO buyer = `1 - max(no_dollars_fp.price)`
- **Spread** = best_ask − best_bid (tight spread ⇒ the two contracts together cost close to $1.00)
- **Mid** = (best_bid + best_ask) / 2 — the market-implied probability of YES

### Displaying YES and NO panels

The visualiser in `orderbook/examples/kalshi_orderbook.rs` keeps a single two-sided book internally but renders two panels side-by-side:

- **UP (YES panel)** — book as-is (YES bids on the left, YES asks on the right).
- **DOWN (NO panel)** — the mirror view:
  ```
  no_bid(p) = 1 - yes_ask(p)
  no_ask(p) = 1 - yes_bid(p)
  no_mid    = 1 - yes_mid
  ```
  Both panels always show the same spread, because the spread is a property of the combined market, not of either contract in isolation.

This matches the dual orderbook UI on Kalshi's website.

---

## Configuration

### Credentials (`credentials/kalshi.yaml`)

```yaml
kalshi:
  key_id: "<your-key-id-uuid>"       # API key UUID from kalshi.com/account/profile/api-keys
  private_key: |
    -----BEGIN PRIVATE KEY-----
    <your-rsa-private-key-pem>
    -----END PRIVATE KEY-----
```

Pass to the visualiser with `--credentials credentials/kalshi.yaml` (default path). This file is separate from the display config so secrets stay out of version control.

Kalshi uses **RSA-PSS / SHA-256** authentication. Each WebSocket upgrade request is signed with the private key; the `key_id` UUID identifies which public key Kalshi should verify against. Both fields are mandatory.

### Visualiser config (`configs/kalshi.yaml`)

```yaml
server:
  render_interval: 300    # terminal redraw interval in milliseconds

storage:
  depth: 10               # orderbook levels per side

kalshi:
  market:
    - enable: true
      series: "KXBTC15M"
      interval_secs: 900
    - enable: true
      series: "KXETH15M"
      interval_secs: 900
```

| Field | Type | Description |
|-------|------|-------------|
| `server.render_interval` | u64 | Terminal redraw interval in milliseconds |
| `storage.depth` | usize | Orderbook levels per side to display |
| `kalshi.market[].enable` | bool | Skip this entry if false |
| `kalshi.market[].series` | string | Kalshi series ticker (e.g. `KXBTC15M`) |
| `kalshi.market[].interval_secs` | u64 | Window size in seconds (900 = 15 min) |

---

## Running the Visualiser

The `kalshi_orderbook` example renders a live terminal UI showing YES/NO orderbooks for BTC and ETH, auto-rotating every 15 minutes.

```bash
# Config file (recommended)
cargo run --example kalshi_orderbook --release -- \
    --config configs/kalshi.yaml \
    --credentials credentials/kalshi.yaml

# CLI flags
cargo run --example kalshi_orderbook --release -- \
    --series KXBTC15M \
    --series KXETH15M \
    --depth 8 \
    --credentials credentials/kalshi.yaml

# Single series with custom rotation timing
cargo run --example kalshi_orderbook --release -- \
    --config configs/kalshi.yaml \
    --early-start-secs 30 \
    --credentials credentials/kalshi.yaml
```

### Terminal display

Each series occupies one panel, split left (YES/bids) and right (NO/asks):

```
┌ BTC 15m ↑↓ KXBTC15M-26APR130415-15 ──────────────────────────────┐
│          UP  (Yes)          │         DOWN  (No)                  │
│   price       qty           │   price       qty                   │
│  0.7200    1234.00          │  0.3100     567.00                  │
│  0.7100     890.00          │  0.3200     234.00                  │
├─── sprd=0.0100 ─────────────┼─── sprd=0.0100 ────────────────────┤
│  0.6900     345.00          │  0.3400     678.00                  │
│  0.6800     123.00          │  0.3500     901.00                  │
└───────────────────────────────────────────────────────────────────┘
```

### Auto-rotation scheduler

Each series runs an independent scheduler loop:

1. Compute the current window's close time from the wall clock.
2. Build the event ticker (`build_event_ticker`) and resolve the full market ticker via REST (`resolve_market_ticker`). On REST failure, wait 5s and retry.
3. If it differs from the currently-connected ticker, spawn a new `MarketTask` and drop the old one.
4. Sleep until `early_start_secs` before close (sampling `Utc::now()` fresh, so WebSocket handshake time during step 3 is accounted for).
5. Resolve the next window's market ticker and pre-start its connection (both run briefly in parallel).
6. Sleep the remainder until the current window expires (again with a fresh `Utc::now()` sample).
7. Drop the old connection; promote the pre-started task to current; repeat.

The panel label updates automatically to show the current ticker. The panel itself stays stable across rotations (same series key throughout).

---

## Wire Format Reference

Added to `docs/exchange-wire-formats.md` for completeness. Key differences from other exchanges:

| Property | Kalshi | Polymarket | Hyperliquid |
|----------|--------|------------|-------------|
| Auth | RSA-PSS signed upgrade headers | None | None |
| Ping | Server-initiated (client pongs) | None | Client-initiated |
| Book update | Signed dollar delta | Absolute size | Full snapshot |
| Sides on wire | Two bid-ladders (YES, NO) | token-per-side | bids/asks |
| Book as rendered | Combined into YES-perspective two-sided book (NO complemented) | per-token | bids/asks |
| Price unit | USD (0.00–1.00) | USDC (0.00–1.00) | USD |
| Ticker | Event from clock + strike suffix resolved via REST | Token ID (256-bit) | Coin name |

---

## Redis Integration (orderbook_server)

When `orderbook_server` runs with `redis.enabled: true`, every Kalshi window publishes its live state to Redis so the HTTP backend (`backend/`) and frontend can render markets without subscribing to the ZMQ stream. The architecture mirrors the [Polymarket Redis integration](polymarket.md#redis-integration-orderbook_server) — a scheduler owns window lifecycle, a per-window engine owns Redis writes, and a watchdog handles cleanup at shutdown.

### Key Schema

All keys are constructed by `RedisKey` (`libs/src/redis_client/keys.rs`).

| Key | Type | Lifetime | Written by |
|-----|------|----------|------------|
| `ob:kalshi:{ticker}` | string (msgpack) | overwritten each tick | per-window engine snapshot hook |
| `bba:kalshi:{ticker}` | string (msgpack) | overwritten each tick | per-window engine snapshot hook |
| `snapshots:kalshi:{ticker}` | list (msgpack) | LPUSH + LTRIM to `snapshot_cap` | per-window engine snapshot hook |
| `trades:kalshi:{ticker}` | list (msgpack) | LPUSH + LTRIM to `trade_cap` | per-window engine trade hook |
| `kalshi:label:{ticker}` | hash | one entry per live ticker | window setup |
| `kalshi:label:index` | set of `ticker` | mirrors live labels | window setup |
| `kalshi:series:{series}:tickers` | set of `ticker` | per series | window setup |

`{ticker}` is the full Kalshi market ticker (e.g. `KXBTC15M-26APR160700-00`), which is also what the orderbook publishes as `symbol`. There is no UP/DOWN pairing in Redis — Kalshi gives one combined two-sided book per market (see [Orderbook mechanism](#orderbook-mechanism)), so each window contributes exactly **one** label.

The `kalshi:label:{ticker}` hash:

```
series         = "KXBTC15M"
ticker         = "KXBTC15M-26APR160700-00"
window_start   = "1776681900"   # Unix seconds — UTC :00/:15/:30/:45 boundary
interval_secs  = "900"          # always 900 for the rolling 15-min markets
```

> **`window_start` derivation.** Kalshi windows are clock-aligned and always 15 minutes long, so `window_start = (now + 30) / interval * interval`. The `+30` jumps safely past the pre-start overlap (~10s before close), so the value resolves to the new window's boundary rather than the previous one's. Because the boundary is deterministic, no parsing of `ticker` is needed (unlike Polymarket where `window_start` lives in the `full_slug`).

### Window Setup (`spawn_kalshi_window` in `zmq_server/src/setup.rs`)

When the scheduler fires for a new `ticker`:

1. **Build the connection config** with the resolved `ticker`, WebSocket URL, and signed credentials.
2. **Evict expired prior-window keys** for this `series`. For each `prior_ticker` in `kalshi:series:{series}:tickers` other than the current one:
   - Read its `kalshi:label:{prior_ticker}` hash; treat it as stale if `interval_secs == 0` (legacy/missing) or `window_start + interval_secs <= now`.
   - If stale: `DEL` `kalshi:label`, `ob`, `bba`, `snapshots`, `trades`; `SREM` from `kalshi:label:index` and the series set.
   - **Active overlapping windows are skipped** — they are owned by their own task and clean up when they exit.
3. **Register the new ticker** under `kalshi:series:{series}:tickers`.
4. **Write `kalshi:label:{ticker}`** and add to `kalshi:label:index`.
5. **Spin up a per-window engine** (separate `StreamEngine` from the main one) and call `attach_redis(...)` so its snapshots/trades flow into Redis under the keys above.
6. **Spawn a watchdog task** that polls `SystemControl.is_shutdown()`. When the scheduler stops this window via `WindowTask::stop()` → `handle.abort()`, the abort cancels any await past it, so cleanup cannot live inline after `system.run().await`. The watchdog runs independently and `DEL`s/`SREM`s every key tied to this ticker.

### Backend Read Path (`backend/src/lib.rs::get_kalshi_markets`)

The `GET /api/kalshi/markets` handler:

1. Reads `kalshi:label:index`.
2. For each `ticker`, loads `kalshi:label:{ticker}`. Empty hash → orphan index entry, `SREM` and skip.
3. **Lazy expiry:** if `interval_secs > 0 && window_start + interval_secs <= now`, the label is past its window. Delete `kalshi:label`, `ob`, `bba`, `snapshots`, `trades` and remove from the index. This is a backstop for the scheduler eviction in case a window's watchdog never ran (e.g. process killed).
4. Sort by `(series, window_start, ticker)` and return.

Response shape:

```json
[
  {
    "ticker": "KXBTC15M-26APR160700-00",
    "series": "KXBTC15M",
    "window_start": 1776681900,
    "interval_secs": 900,
    "title": "KXBTC15M-26APR160700-00"
  }
]
```

### Why Three Cleanup Paths?

| Path | Trigger | Owner | Purpose |
|------|---------|-------|---------|
| Eviction loop in window setup | New window starts | Scheduler | Remove the previous window's keys at rollover |
| Watchdog | `SystemControl.shutdown()` from `WindowTask::stop()` | Per-window task | Remove this window's keys when the scheduler tells it to stop |
| Backend lazy expiry | API request hits a stale label | Backend handler | Backstop: clean up if the orderbook_server crashed or was restarted before the watchdog ran |

Steady-state invariant:

```
COUNT kalshi:label:* == SCARD kalshi:label:index
                     == COUNT ob:kalshi:*
                     == COUNT bba:kalshi:*
                     == COUNT snapshots:kalshi:*
                     == COUNT trades:kalshi:*
                     == (# enabled series)         # one ticker per series per window
```

A transient overlap of one extra ticker per series is expected during pre-start (~10s before window close).

### Health-check command

```bash
docker compose exec -T redis sh -c '
labels=$(redis-cli SMEMBERS kalshi:label:index | sort)
for prefix in ob bba snapshots trades; do
  ids=$(redis-cli --scan --pattern "$prefix:kalshi:*" | sed "s|$prefix:kalshi:||" | sort)
  echo "--- orphan $prefix (no label) ---"; comm -23 <(echo "$ids") <(echo "$labels")
done
echo "--- label has no ob ---"
obs=$(redis-cli --scan --pattern "ob:kalshi:*" | sed "s|ob:kalshi:||" | sort)
comm -13 <(echo "$obs") <(echo "$labels")
'
```

### Configuration

```yaml
redis:
  enabled: true
  url: "redis://127.0.0.1:6379"
  snapshot_cap: 1000   # LTRIM cap for snapshots:kalshi:{ticker}
  trade_cap: 500       # LTRIM cap for trades:kalshi:{ticker}
  event_list_cap: 5000 # LTRIM cap for events:fills, events:orders
```

The caps are per-key, not global, so total memory scales with the number of currently-live tickers. Once eviction is working correctly that is exactly `# enabled series`, plus a transient extra per series during pre-start.

### Frontend

The Kalshi page at `/kalshi` (`frontend/src/pages/Kalshi.tsx`) polls `/api/kalshi/markets` every 5s and renders one panel per live ticker, showing best bid / spread / best ask plus a depth-bar visualisation of the top-12 levels per side. The panel header shows the series and the window close time (UTC).
