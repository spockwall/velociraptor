# Exchange Wire Formats

Raw WebSocket message formats for each supported exchange, as received before parsing.
Use this as a reference when adding new exchanges or debugging parse errors.

---
## Normalised output (`OrderbookUpdate`)

All parsers produce the same internal type regardless of exchange:

```rust
pub struct OrderbookUpdate {
    pub action: OrderbookAction,   // Snapshot | Update | Delete
    pub orders: Vec<GenericOrder>,
    pub symbol: String,            // exchange-native symbol key
    pub timestamp: DateTime<Utc>,
    pub exchange: ExchangeName,
}

pub struct GenericOrder {
    pub price: f64,
    pub qty: f64,
    pub side: String,              // "Bid" or "Ask"
    pub symbol: String,
    pub timestamp: String,         // RFC 3339
}
```

### Symbol key format in `OrderbookEngine`

The engine stores orderbooks in a `DashMap` keyed by `"<exchange>:<SYMBOL>"`:

| Exchange | Key example |
|----------|-------------|
| Binance | `binance:BTCUSDT` |
| OKX | `okx:BTC-USDT` |
| Polymarket | `polymarket:75467129...` |
| Hyperliquid | `hyperliquid:BTC` |
| Kalshi | `kalshi:KXBTC15M-26APR130415-15` |

This is also the ZMQ topic format clients subscribe to.

---

## Binance

**Endpoint:** `wss://fstream.binance.com/ws`
**Subscription:** one message covering all symbols
**Ping:** standard WebSocket binary ping frame (no custom application ping)
**Symbol format:** lowercase pair — `btcusdt`, `ethusdt`

### Subscription message

```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@depth20@100ms", "ethusdt@depth20@100ms"],
  "id": 1
}
```

### Subscription ack

```json
{ "result": null, "id": 1 }
```

### Book update (`@depth20@100ms`)

Full 20-level snapshot delivered every 100ms. There are no incremental diffs — every message replaces the book.

```json
{
  "s": "BTCUSDT",
  "b": [
    ["97500.10", "0.500"],
    ["97499.00", "1.200"]
  ],
  "a": [
    ["97501.00", "0.300"],
    ["97502.50", "2.100"]
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `s` | string | Symbol (uppercase) |
| `b` | `[[price, qty]]` | Bids, best first |
| `a` | `[[price, qty]]` | Asks, best first |

**Note:** no timestamp in the message — the parser uses `Utc::now()`.
**Action:** always `OrderbookAction::Snapshot`.

---

## OKX

**Endpoint:** `wss://ws.okx.com:8443/ws/v5/public`
**Subscription:** one message covering all symbols
**Ping:** `{"op":"ping"}` → server responds `{"op":"pong"}`
**Symbol format:** `BTC-USDT` (SPOT), `BTC-USDT-SWAP` (perp), `BTC-USDT-231229` (future)

### Subscription message

```json
{
  "op": "subscribe",
  "args": [
    { "channel": "books", "instId": "BTC-USDT" },
    { "channel": "books", "instId": "ETH-USDT" }
  ]
}
```

### Subscription ack

```json
{ "event": "subscribe", "arg": { "channel": "books", "instId": "BTC-USDT" } }
```

### Snapshot message (`action: "snapshot"`)

Sent once on subscribe. Full book state.

```json
{
  "action": "snapshot",
  "arg": { "channel": "books", "instId": "BTC-USDT" },
  "data": [{
    "asks": [["97501.0", "0.5", "0", "1"]],
    "bids": [["97500.0", "1.2", "0", "2"]],
    "ts": "1711900000000",
    "checksum": 123456789,
    "seqId": 10001,
    "prevSeqId": -1
  }]
}
```

### Update message (`action: "update"`)

Incremental diffs after the initial snapshot.

```json
{
  "action": "update",
  "arg": { "channel": "books", "instId": "BTC-USDT" },
  "data": [{
    "asks": [["97501.0", "0", "0", "0"]],
    "bids": [["97500.0", "1.5", "0", "2"]],
    "ts": "1711900000100",
    "checksum": 987654321,
    "seqId": 10002,
    "prevSeqId": 10001
  }]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `asks`/`bids` | `[[price, qty, num_liquidated, num_orders]]` | Size `"0"` → remove level |
| `ts` | string (Unix ms) | Event timestamp |
| `seqId` | int | Server sequence number |
| `checksum` | int | CRC32 checksum for book validation |

**Action:** `snapshot` message → `OrderbookAction::Snapshot`; `update` message → `OrderbookAction::Update` (or `Delete` when qty == 0).

---

## Polymarket

**Endpoint:** `wss://ws-subscriptions-clob.polymarket.com/ws/market`
**Subscription:** one message covering all asset IDs
**Ping:** none (standard WebSocket keep-alive is sufficient)
**Symbol format:** numeric token ID string (77 digits) — one per outcome (Yes / No)

### Subscription message

```json
{
  "assets_ids": [
    "75467129615908319583031474642658885479135630431889036121812713428992454630178",
    "38429637202672672869706423368607527823026446801565350617000393884056521296910"
  ],
  "type": "market"
}
```

**Important:** always subscribe to both Yes and No token IDs together. If only one is subscribed, `price_change` events arrive for the other token before its snapshot, causing "update before snapshot" warnings.

### Subscription ack

```json
[]
```

An empty JSON array. The parser ignores it.

### All messages are JSON arrays

Every server message is wrapped in a JSON array: `[{...event...}]`. The parser unwraps the array before processing.

### `book` event — full snapshot

Sent on initial subscribe and after each trade.

```json
[{
  "event_type": "book",
  "asset_id": "75467129615908319583031474642658885479135630431889036121812713428992454630178",
  "market": "0xbd31dc8a20211944f6b70f31557f1001557b59905b7738480ca09bd4532f84af",
  "bids": [
    { "price": "0.087", "size": "37.67" },
    { "price": "0.085", "size": "100.00" }
  ],
  "asks": [
    { "price": "0.089", "size": "74.93" },
    { "price": "0.091", "size": "50.00" }
  ],
  "timestamp": "1711900000000",
  "hash": "0xabc123..."
}]
```

**Action:** `OrderbookAction::Snapshot`.

### `price_change` event — incremental diff

Sent on order placement or cancellation. One message can contain changes for **multiple assets** (both Yes and No tokens).

```json
[{
  "event_type": "price_change",
  "market": "0xbd31dc8a...",
  "timestamp": "1711900000123",
  "price_changes": [
    {
      "asset_id": "75467129...",
      "price": "0.088",
      "size": "50.00",
      "side": "BUY",
      "best_bid": "0.088",
      "best_ask": "0.089"
    },
    {
      "asset_id": "38429637...",
      "price": "0.912",
      "size": "0",
      "side": "SELL",
      "best_bid": "0.910",
      "best_ask": "0.912"
    }
  ]
}]
```

| Field | Values | Description |
|-------|--------|-------------|
| `side` | `"BUY"` / `"SELL"` | `BUY` = bid side, `SELL` = ask side |
| `size` | string | `"0"` → remove price level |

**Action:** `size == "0"` → `OrderbookAction::Delete`; otherwise `OrderbookAction::Update`.
The parser groups changes by `asset_id` and emits one `OrderbookUpdate` per asset.

### Other event types (ignored)

`tick_size_change`, `last_trade_price`, `best_bid_ask`, `new_market`, `market_resolved` — all silently skipped.

---

## Hyperliquid

**Endpoint:** `wss://api.hyperliquid.xyz/ws`
**Subscription:** one message **per coin** — sent as `subscription_message` + `post_subscription_messages`
**Ping:** `{"method":"ping"}` every 45s → server responds `{"channel":"pong"}`
**Symbol format:** uppercase coin name only — `BTC`, `ETH`, `SOL` (no quote currency)

### Subscription message (one per coin)

```json
{
  "method": "subscribe",
  "subscription": { "type": "l2Book", "coin": "BTC" }
}
```

### Subscription ack

```json
{ "channel": "subscriptionResponse", "data": { "method": "subscribe", ... } }
```

### `l2Book` message — full snapshot

Sent on every book change. Every message is a complete replacement of the book — there are no incremental diffs.

```json
{
  "channel": "l2Book",
  "data": {
    "coin": "BTC",
    "levels": [
      [
        { "px": "97500.0", "sz": "0.5", "n": 2 },
        { "px": "97499.0", "sz": "1.2", "n": 1 }
      ],
      [
        { "px": "97501.0", "sz": "0.3", "n": 1 },
        { "px": "97502.0", "sz": "2.1", "n": 3 }
      ]
    ],
    "time": 1711900000123
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `coin` | string | Uppercase coin name |
| `levels[0]` | array of `WsLevel` | Bids, best first |
| `levels[1]` | array of `WsLevel` | Asks, best first |
| `time` | u64 | Unix millisecond timestamp (number, not string) |
| `px` | string | Price |
| `sz` | string | Size |
| `n` | u64 | Number of orders at this level |

**Action:** always `OrderbookAction::Snapshot`.

### Pong message

```json
{ "channel": "pong" }
```

---

## Kalshi

**Endpoint:** `wss://api.elections.kalshi.com/trade-api/ws/v2?apiKey=<key>`
**Authentication:** API key required as query parameter — get one at https://kalshi.com/account/profile/api-keys
**Subscription:** one message covering all tickers
**Ping:** server-initiated — Kalshi sends `{"type":"ping"}` every ~10s; parser detects via `is_ping()`, infrastructure sends WebSocket pong automatically
**Symbol format:** full window ticker — `KXBTC15M-26APR130415-15`

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

### Subscription ack

```json
{ "type": "subscribed", "sid": 1, "seq": 1, "msg": {} }
```

Logged as info. No `OrderbookMessage` emitted.

### `orderbook_snapshot` — full book state

```json
{
  "type": "orderbook_snapshot",
  "sid": 2,
  "seq": 2,
  "msg": {
    "market_ticker": "KXBTC15M-26APR130415-15",
    "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
    "yes_dollars_fp": [
      ["0.6700", "24.64"],
      ["0.6800", "400.00"]
    ],
    "no_dollars_fp": [
      ["0.3200", "100.00"],
      ["0.3300", "250.00"]
    ]
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `market_ticker` | string | Used as orderbook `symbol` |
| `yes_dollars_fp` | `[[price, size]]` | YES contracts — mapped to **bid** side |
| `no_dollars_fp` | `[[price, size]]` | NO contracts — mapped to **ask** side |

Prices in USD (0.0–1.0 range). Sizes in dollar amounts.

**Action:** `OrderbookAction::Snapshot`

### `orderbook_delta` — incremental update

```json
{
  "type": "orderbook_delta",
  "sid": 2,
  "seq": 3,
  "msg": {
    "market_ticker": "KXBTC15M-26APR130415-15",
    "market_id": "9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
    "price_dollars": "0.6800",
    "delta_fp": "-54.00",
    "side": "yes",
    "ts": "2026-04-13T08:07:01Z"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `price_dollars` | string | Price level being updated |
| `delta_fp` | string | **Signed change** in dollar size. Not an absolute value. |
| `side` | string | `"yes"` → bid side; `"no"` → ask side |
| `ts` | string | RFC3339 timestamp (optional) |

**Action:** `delta_fp > 0` → `OrderbookAction::Update`; `delta_fp ≤ 0` → `OrderbookAction::Delete` (size 0 sent, engine removes the level)

### Server ping (ignored by emitter, answered by infrastructure)

```json
{ "type": "ping", "id": 42 }
```

---

