---
name: velociraptor-add-exchange
description: Step-by-step guide for adding a new exchange connector to velociraptor — every file to touch, the trait shapes, and a final checklist. Use when wiring up a brand-new venue.
---

# Velociraptor — Adding a New Exchange

Follow steps in order; the compiler catches missed exhaustive match arms.

## 1. Register the exchange name — `libs/src/protocol/mod.rs`

```rust
pub enum ExchangeName {
    Okx, Binance, Polymarket, Hyperliquid, Kalshi,
    MyExchange,   // ADD
}

impl ExchangeName {
    pub fn to_str(&self) -> &'static str {
        match self { ExchangeName::MyExchange => "myexchange", /* ... */ }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "myexchange" => Some(ExchangeName::MyExchange),
            _ => None,
        }
    }
}
```

## 2. WebSocket URL — `orderbook/src/types/endpoints.rs`

```rust
pub mod myexchange {
    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://ws.myexchange.com/stream";
    }
}
```

## 3. Wire URL into `ClientConfig` — `orderbook/src/connection/configs.rs`

```rust
use crate::types::endpoints::myexchange;

let (ws_url, ping_interval) = match name {
    ExchangeName::Binance    => (binance::ws::PUBLIC_STREAM,    15u64),
    ExchangeName::MyExchange => (myexchange::ws::PUBLIC_STREAM, 30u64),
    // ...
};
```

## 4. Create the exchange module — `orderbook/src/exchanges/myexchange/`

### 4a. `types.rs` — wire structs (deserialise-only, private)

```rust
#[derive(Debug, Deserialize)]
pub struct MyLevel { pub price: String, pub size: String }

#[derive(Debug, Deserialize)]
pub struct MyBookEvent {
    pub symbol: String,
    pub bids: Vec<MyLevel>,
    pub asks: Vec<MyLevel>,
    pub timestamp: u64,
}
```

### 4b. `subscription.rs` — builder

Single message (Binance/OKX style):

```rust
pub struct MyExchangeSubMsgBuilder { symbols: Vec<String> }

impl MyExchangeSubMsgBuilder {
    pub fn new() -> Self { Self { symbols: vec![] } }
    pub fn with_symbol(mut self, sym: &str) -> Self { self.symbols.push(sym.into()); self }
    pub fn build(self) -> String {
        serde_json::json!({ "op": "subscribe", "args": self.symbols }).to_string()
    }
}
```

One message per symbol (Hyperliquid style — returns `Vec<String>`, use `set_subscription_messages` plural setter):

```rust
pub fn build(self) -> Vec<String> {
    self.symbols.iter()
        .map(|s| serde_json::json!({
            "method": "subscribe",
            "subscription": { "type": "l2Book", "coin": s }
        }).to_string())
        .collect()
}
```

### 4c. `msg_parser.rs` — `MsgParserTrait`

```rust
pub struct MyExchangeMessageParser;

impl MsgParserTrait<StreamMessage> for MyExchangeMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        // 1. Deserialise envelope
        // 2. Match on event type
        // 3. Build Vec<GenericOrder> from bids/asks
        // 4. Return StreamMessage::OrderbookUpdate(...)
        todo!()
    }
    fn build_ping(&self) -> Option<String> { Some(r#"{"op":"ping"}"#.into()) }  // None = bare WS ping
    fn is_pong(&self, text: &str) -> bool { text.contains("\"pong\"") }
    fn is_ping(&self, text: &str) -> bool { text.contains("\"ping\"") }
}
```

`OrderbookAction`:

| Action | When |
|---|---|
| `Snapshot` | Message replaces entire book |
| `Update` | Updates individual price levels (qty = new total) |
| `Delete` | Price level removed (qty = 0) |

`GenericOrder` fields: `price: f64, qty: f64, side: String("Bid"/"Ask"), symbol: String, timestamp: String (rfc3339)`.

Timestamp helpers:

```rust
let ts = Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now);
// or for string ms:
use libs::time::parse_timestamp_ms;
let ts = parse_timestamp_ms(&event.timestamp_str);
```

> **Critical rule: emit one `StreamMessage` per price-level change.** Never batch levels with different actions into one message — mixed Update/Delete batches cause silent data loss. See `polymarket/msg_parser.rs` for the correct pattern.

### 4d. `client.rs` — connection wrapper

```rust
pub struct MyExchangeClient {
    inner: ClientBase<MyExchangeMessageParser, StreamMessage>,
}

impl MyExchangeClient {
    pub fn new(config: ClientConfig, message_tx: UnboundedSender<StreamMessage>,
               system_control: SystemControl) -> Self {
        let inner = ClientBase::new(
            config, message_tx, system_control,
            MyExchangeMessageParser, ExchangeName::MyExchange,
            None,   // optional header builder (Kalshi uses RSA auth)
        );
        Self { inner }
    }
}

impl ClientTrait for MyExchangeClient {
    async fn run(&mut self) -> anyhow::Result<()> { self.inner.run().await }
}
```

`ClientBase` sends each string in `config.subscription_messages` as a separate WS text frame on connect — no per-symbol splitting needed at the client layer.

### 4e. `mod.rs` — re-exports

```rust
pub mod client;
pub mod msg_parser;
pub mod subscription;
pub mod types;

pub use client::MyExchangeClient;
pub(crate) use msg_parser::MyExchangeMessageParser;
pub use subscription::MyExchangeSubMsgBuilder;
```

## 5. Factory — `orderbook/src/exchanges/mod.rs`

```rust
pub mod myexchange;
use crate::exchanges::myexchange::MyExchangeClient;

// in spawn_connection():
ExchangeName::MyExchange => {
    let mut c = MyExchangeClient::new(config, message_tx, system_control);
    tokio::spawn(async move { let _ = c.run().await })
}
```

## 6. Public API — `orderbook/src/lib.rs`

```rust
pub use exchanges::myexchange::{MyExchangeClient, MyExchangeSubMsgBuilder};
```

## 7. Config + server wiring

**`libs/src/configs/`** — add struct:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MyExchangeConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}
```

Add `pub myexchange: MyExchangeConfig` to top-level `Config`.

**`zmq_server/src/setup.rs`** — wiring helper:

```rust
pub fn add_myexchange(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() { return false; }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::MyExchange)
            .set_subscription_message(MyExchangeSubMsgBuilder::new().with_symbols(&refs).build()),
    );
    info!(symbols = ?symbols, "MyExchange enabled");
    true
}
```

**`zmq_server/src/bin/orderbook_server.rs`** — call helper:

```rust
let has_static = [
    cfg.binance.enabled     && add_binance(&mut system_cfg, &cfg.binance.symbols),
    cfg.okx.enabled         && add_okx(&mut system_cfg, &cfg.okx.symbols),
    cfg.hyperliquid.enabled && add_hyperliquid(&mut system_cfg, &cfg.hyperliquid.coins),
    cfg.myexchange.enabled  && add_myexchange(&mut system_cfg, &cfg.myexchange.symbols), // ADD
].iter().any(|&v| v);
```

**`configs/dev/config.yaml`**:

```yaml
myexchange:
  enabled: false
  symbols: ["BTC", "ETH"]
```

## Checklist

- [ ] `libs/src/protocol/mod.rs` — `ExchangeName` variant + `to_str` + `from_str`
- [ ] `orderbook/src/types/endpoints.rs` — WebSocket URL constant
- [ ] `orderbook/src/connection/configs.rs` — URL match arm (+ ping_interval if non-default)
- [ ] `orderbook/src/exchanges/myexchange/types.rs`
- [ ] `orderbook/src/exchanges/myexchange/subscription.rs`
- [ ] `orderbook/src/exchanges/myexchange/msg_parser.rs`
- [ ] `orderbook/src/exchanges/myexchange/client.rs`
- [ ] `orderbook/src/exchanges/myexchange/mod.rs`
- [ ] `orderbook/src/exchanges/mod.rs` — factory match arm
- [ ] `orderbook/src/lib.rs` — public re-exports
- [ ] `libs/src/configs/` — config struct + add to `Config`
- [ ] `zmq_server/src/setup.rs` — `add_myexchange` helper
- [ ] `zmq_server/src/bin/orderbook_server.rs` — call helper
- [ ] `configs/dev/config.yaml` — config section
- [ ] `cargo check --workspace` — fix any exhaustive match errors

## Reference matrix

| Exchange | Snapshot type | Multi-symbol | Custom ping | Notes |
|---|---|---|---|---|
| Binance | Partial depth (incremental) | Single JSON array | None (WS ping) | `@depth20@100ms` stream |
| OKX | Snapshot + incremental | Single JSON array | `{"op":"ping"}` | |
| Polymarket | Full snapshot + per-level diffs | Per-asset in one message | `"PING"` text | Token IDs as symbols; one `StreamMessage` per change |
| Hyperliquid | Full snapshot every update | `Vec<String>` (one frame per coin) | `{"method":"ping"}` | Use `set_subscription_messages` |
| Kalshi | Snapshot + signed dollar delta | Single message | Server-initiated (client pongs) | RSA-PSS WS upgrade auth; ticker resolved via REST |

## Design rules

- **One `StreamMessage` per price-level change.** Never batch mixed actions.
- **`ExchangeName` lives in `libs::protocol`**, not in `orderbook`.
- **Wiring helpers live in `zmq_server/src/setup.rs`**, not in binaries. Binaries call helpers; helpers call `cfg.with_exchange(...)`.
- **Rolling-window schedulers** (Polymarket, Kalshi) live in `orderbook/src/exchanges/{exchange}/scheduler.rs` and expose `run_rolling_scheduler` + `WindowTask`. Binaries call `spawn_{exchange}_schedulers` from `setup.rs`.
