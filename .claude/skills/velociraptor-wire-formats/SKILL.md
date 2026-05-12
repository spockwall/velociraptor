---
name: velociraptor-wire-formats
description: Raw WebSocket message formats for every supported exchange (Binance, OKX, Polymarket, Hyperliquid, Kalshi) — endpoints, subscription messages, snapshot/update payloads, normalised output types. Use when adding an exchange or debugging parser issues.
---

# Velociraptor — Exchange Wire Formats

## Normalised output (`OrderbookUpdate`)

All parsers produce the same internal type:

```rust
pub struct OrderbookUpdate {
    pub action: OrderbookAction,   // Snapshot | Update | Delete
    pub orders: Vec<GenericOrder>,
    pub symbol: String,            // exchange-native key
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

`OrderbookEngine` stores books in `DashMap` keyed `"<exchange>:<SYMBOL>"`:

| Exchange | Key example |
|---|---|
| Binance | `binance:BTCUSDT` |
| OKX | `okx:BTC-USDT` |
| Polymarket | `polymarket:75467129...` |
| Hyperliquid | `hyperliquid:BTC` |
| Kalshi | `kalshi:KXBTC15M-26APR130415-15` |

Same string is the ZMQ topic format.

---

## Binance

- **Endpoint:** `wss://fstream.binance.com/ws`
- **Sub:** one message covering all symbols
- **Ping:** standard WS binary ping (no app-level ping)
- **Symbol format:** lowercase pair — `btcusdt`

**Subscribe:**
```json
{"method":"SUBSCRIBE","params":["btcusdt@depth20@100ms","ethusdt@depth20@100ms"],"id":1}
```

**Ack:** `{"result":null,"id":1}`

**Book update (`@depth20@100ms`):** full 20-level snapshot every 100ms (no diffs).

```json
{"s":"BTCUSDT","b":[["97500.10","0.500"]],"a":[["97501.00","0.300"]]}
```

| Field | Type | Description |
|---|---|---|
| `s` | str | Uppercase symbol |
| `b` | `[[p,q]]` | Bids, best first |
| `a` | `[[p,q]]` | Asks, best first |

No timestamp in message → parser uses `Utc::now()`. Action: always `Snapshot`.

---

## OKX

- **Endpoint:** `wss://ws.okx.com:8443/ws/v5/public`
- **Sub:** one message all symbols
- **Ping:** `{"op":"ping"}` → server `{"op":"pong"}`
- **Symbol format:** `BTC-USDT` (SPOT), `BTC-USDT-SWAP` (perp), `BTC-USDT-231229` (future)

**Subscribe:**
```json
{"op":"subscribe","args":[
  {"channel":"books","instId":"BTC-USDT"},
  {"channel":"books","instId":"ETH-USDT"}
]}
```

**Ack:** `{"event":"subscribe","arg":{...}}`

**Snapshot (`action:"snapshot"`):** sent once on subscribe.

```json
{"action":"snapshot","arg":{"channel":"books","instId":"BTC-USDT"},
 "data":[{"asks":[["97501.0","0.5","0","1"]],"bids":[["97500.0","1.2","0","2"]],
          "ts":"1711900000000","checksum":123456789,"seqId":10001,"prevSeqId":-1}]}
```

**Update (`action:"update"`):** incremental diffs.

| Field | Description |
|---|---|
| `asks`/`bids` | `[[price, qty, num_liquidated, num_orders]]`; qty `"0"` → remove level |
| `ts` | Unix ms (string) |
| `seqId` / `checksum` | Sequence + CRC32 for validation |

Action: `snapshot` → `Snapshot`; `update` → `Update` (or `Delete` when qty == 0).

---

## Polymarket

- **Endpoint:** `wss://ws-subscriptions-clob.polymarket.com/ws/market`
- **Sub:** one message all asset IDs
- **Ping:** none (standard WS keep-alive)
- **Symbol format:** numeric token ID string (77 digits) per outcome (Yes / No)

**Always subscribe to both Yes and No token IDs together** — solo subscriptions cause `price_change` for the unsubscribed side to arrive before its snapshot.

**Subscribe:**
```json
{"assets_ids":["754671296...","384296372..."],"type":"market"}
```

**Ack:** `[]` (empty array, ignored).

**All server messages are JSON arrays** `[{...}]` — parser unwraps before processing.

**`book` event — full snapshot:**
```json
[{"event_type":"book","asset_id":"754671296...","market":"0x...",
  "bids":[{"price":"0.087","size":"37.67"}],
  "asks":[{"price":"0.089","size":"74.93"}],
  "timestamp":"1711900000000","hash":"0x..."}]
```

Action: `Snapshot`.

**`price_change` event — incremental:** one message can contain changes for multiple assets (both tokens).

```json
[{"event_type":"price_change","market":"0x...","timestamp":"1711900000123",
  "price_changes":[
    {"asset_id":"754671296...","price":"0.088","size":"50.00","side":"BUY",
     "best_bid":"0.088","best_ask":"0.089"},
    {"asset_id":"384296372...","price":"0.912","size":"0","side":"SELL",
     "best_bid":"0.910","best_ask":"0.912"}
  ]}]
```

| Field | Values |
|---|---|
| `side` | `BUY` = bid, `SELL` = ask |
| `size` | `"0"` → remove price level |

Action: `size=="0"` → `Delete`; else `Update`. Parser groups by `asset_id` and emits **one `OrderbookUpdate` per asset**.

Ignored events: `tick_size_change`, `last_trade_price`, `best_bid_ask`, `new_market`, `market_resolved`.

---

## Hyperliquid

- **Endpoint:** `wss://api.hyperliquid.xyz/ws`
- **Sub:** one message **per coin** (use `subscription_message` + `post_subscription_messages`)
- **Ping:** `{"method":"ping"}` every 45s → server `{"channel":"pong"}`
- **Symbol format:** uppercase coin name only — `BTC`, `ETH`, `SOL`

**Subscribe (one per coin):**
```json
{"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}
```

**`l2Book` event — full snapshot every change:**
```json
{"channel":"l2Book","data":{
  "coin":"BTC",
  "levels":[
    [{"px":"97500.0","sz":"0.5","n":2}],   // bids
    [{"px":"97501.0","sz":"0.3","n":1}]    // asks
  ],
  "time":1711900000123}}
```

| Field | Description |
|---|---|
| `levels[0]` | Bids, best first |
| `levels[1]` | Asks, best first |
| `time` | Unix ms (number, not string) |
| `n` | Number of orders at level |

Action: always `Snapshot`.

---

## Kalshi

- **Endpoint:** `wss://api.elections.kalshi.com/trade-api/ws/v2?apiKey=<key>`
- **Auth:** RSA-PSS signed upgrade headers (see `velociraptor-kalshi`)
- **Sub:** one message all tickers; **channels and tickers as separate arrays** — concatenating `"orderbook_delta:TICKER"` returns a subscribed ack but no data
- **Ping:** server-initiated `{"type":"ping"}`; parser detects via `is_ping()`, infra sends WS pong automatically
- **Symbol format:** full market ticker — `KXBTC15M-26APR130415-15`

**Subscribe:**
```json
{"id":1,"cmd":"subscribe","params":{
  "channels":["orderbook_delta"],
  "market_tickers":["KXBTC15M-26APR160700-00","KXETH15M-26APR160700-00"]
}}
```

**Ack:** `{"type":"subscribed","sid":1,"seq":1,"msg":{}}`

**`orderbook_snapshot`:**
```json
{"type":"orderbook_snapshot","sid":2,"seq":2,"msg":{
  "market_ticker":"KXBTC15M-26APR130415-15",
  "market_id":"9b0f...",
  "yes_dollars_fp":[["0.6700","24.64"]],
  "no_dollars_fp":[["0.3200","100.00"]]
}}
```

| Field | Mapping |
|---|---|
| `yes_dollars_fp` | YES bids → book **Bid** side at `price` |
| `no_dollars_fp` | NO bids → book **Ask** side at `1 - price` (complement) |

Prices in USD (0.0–1.0). Sizes in dollar amounts. Action: `Snapshot`.

**`orderbook_delta`:**
```json
{"type":"orderbook_delta","sid":2,"seq":3,"msg":{
  "market_ticker":"KXBTC15M-26APR130415-15",
  "price_dollars":"0.6800",
  "delta_fp":"-54.00",
  "side":"yes",
  "ts":"2026-04-13T08:07:01Z"
}}
```

| Field | Notes |
|---|---|
| `price_dollars` | Price level being updated |
| `delta_fp` | **Signed change** in dollar size (not absolute) |
| `side` | `yes` → bid side; `no` → ask side (complemented) |
| `ts` | RFC3339 (optional) |

Action: `delta_fp > 0` → `Update`; `delta_fp ≤ 0` → `Delete` (qty=0).

See `velociraptor-kalshi` for the YES+NO complement relation and two-sided book reconstruction.
