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

Each market asks: *is the asset price at window close higher than at window open?*
- YES = price went up → bid side of the orderbook
- NO  = price went down → ask side of the orderbook

Windows are 15 minutes long and aligned to UTC clock boundaries (:00, :15, :30, :45). A new market opens every 15 minutes with a fresh ticker.

---

## Ticker Format

Kalshi market tickers embed the window close time in **US Eastern Time** (EDT/EST, DST-aware):

```
KXBTC15M - 26APR130415 - 15
│          │              │
│          │              └─ suffix (always 15 for 15-min series)
│          └─ close time in ET: YYMONDDHHММ
│             26 Apr 13, 04:15 AM EDT = 08:15 UTC
└─ series ticker
```

**Eastern Time is always used**, not UTC. The scheduler converts UTC → Eastern via `chrono-tz` to handle EDT/EST transitions correctly:

| UTC close time | Eastern time | Ticker segment |
|----------------|--------------|----------------|
| 2026-04-13 08:15 UTC | 04:15 EDT (UTC-4) | `26APR130415` |
| 2026-01-13 10:30 UTC | 05:30 EST (UTC-5) | `26JAN130530` |

The full ticker for the current window is computed deterministically from the wall clock — no API lookup is needed for rotation.

### Computing the current ticker

```
now = Utc::now()
win_close = round_up_to_next_15min_boundary(now)   # UTC
ticker = "{series}-{format_et(win_close)}-15"
```

For example at 08:07 UTC on 2026-04-13:
- `win_close` = 08:15 UTC
- Eastern = 04:15 EDT
- `ticker` = `KXBTC15M-26APR130415-15`

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

```json
{
  "id": 1,
  "cmd": "subscribe",
  "params": {
    "channels": [
      "orderbook_delta:KXBTC15M-26APR130415-15",
      "orderbook_delta:KXETH15M-26APR130415-15"
    ]
  }
}
```

Multiple tickers can be included in one subscribe command. Each channel is prefixed with `orderbook_delta:`.

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
| `yes_dollars_fp` | `[[price, size]]` — YES side, treated as **bids** |
| `no_dollars_fp`  | `[[price, size]]` — NO side, treated as **asks** |

Prices are in US dollars (0.00–1.00 for prediction markets). Sizes are dollar amounts.

**Action:** `OrderbookAction::Snapshot`

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
| `price_dollars` | Price level being updated |
| `delta_fp` | **Signed change** in dollar size. Positive = level grew. Negative = level shrank or was removed. |
| `side` | `"yes"` → bid side; `"no"` → ask side |
| `ts` | RFC3339 timestamp (optional) |

`delta_fp` is a **delta**, not an absolute size. The orderbook engine accumulates these on top of the snapshot.

**Action:** `delta_fp > 0` → `OrderbookAction::Update`; `delta_fp ≤ 0` → `OrderbookAction::Delete` (size set to 0, engine removes the level)

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

1. Compute the current window's close time from the wall clock
2. Derive the ticker deterministically (no API call needed)
3. Connect to the current window's orderbook
4. Sleep until `early_start_secs` before close
5. Pre-start the next window's connection (both run briefly in parallel)
6. When the current window expires, drop the old connection
7. Repeat

The panel label updates automatically to show the current ticker. The panel itself stays stable across rotations (same series key throughout).

---

## Wire Format Reference

Added to `docs/exchange-wire-formats.md` for completeness. Key differences from other exchanges:

| Property | Kalshi | Polymarket | Hyperliquid |
|----------|--------|------------|-------------|
| Auth | API key (query param) | None | None |
| Ping | Server-initiated | None | Client-initiated |
| Book update | Signed delta | Absolute size | Full snapshot |
| Sides | YES=bid, NO=ask | token-per-side | bids/asks |
| Price unit | USD (0.0–1.0) | USDC (0.0–1.0) | USD |
| Ticker | Rolling (date-stamped) | Token ID (256-bit) | Coin name |
