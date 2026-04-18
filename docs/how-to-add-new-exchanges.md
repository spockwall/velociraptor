# Adding a New Exchange

This guide walks through every file you need to touch to add a new exchange connector. Follow the steps in order — the compiler will guide you on any missed exhaustive match arms.

---

## 1. Register the exchange name

**`orderbook/src/types/mod.rs`**

Add a variant to `ExchangeName` and update both conversion methods:

```rust
pub enum ExchangeName {
    Okx,
    Binance,
    Polymarket,
    MyExchange,   // ADD
}

impl ExchangeName {
    pub fn to_str(&self) -> &'static str {
        match self {
            ExchangeName::MyExchange => "myexchange",
            // ...
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "myexchange" => Some(ExchangeName::MyExchange),
            _ => None,
        }
    }
}
```

---

## 2. Add the WebSocket URL

**`orderbook/src/types/endpoints.rs`**

```rust
pub mod myexchange {
    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://ws.myexchange.com/stream";
    }
}
```

---

## 3. Wire up the URL in ClientConfig

**`orderbook/src/connection/configs.rs`**

Add the match arm in `ClientConfig::new()`. Also set `ping_interval` here if your exchange needs something other than the default:

```rust
use crate::types::endpoints::myexchange;

let (ws_url, ping_interval) = match name {
    ExchangeName::Binance    => (binance::ws::PUBLIC_STREAM,    15u64),
    ExchangeName::MyExchange => (myexchange::ws::PUBLIC_STREAM, 30u64),
    // ...
};
```

---

## 4. Create the exchange module

Create `orderbook/src/exchanges/myexchange/` with five files:

### 4a. `types.rs` — wire format structs

Deserialisation-only structs matching the exchange's raw JSON. Keep these private to the module.

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct MyLevel {
    pub price: String,
    pub size:  String,
}

#[derive(Debug, Deserialize)]
pub struct MyBookEvent {
    pub symbol:    String,
    pub bids:      Vec<MyLevel>,
    pub asks:      Vec<MyLevel>,
    pub timestamp: u64,
}
```

### 4b. `subscription.rs` — subscription message builder

Build the JSON string(s) sent on connect.

**Single message covering all symbols** (Binance/OKX style):

```rust
pub struct MyExchangeSubMsgBuilder { symbols: Vec<String> }

impl MyExchangeSubMsgBuilder {
    pub fn new() -> Self { Self { symbols: vec![] } }

    pub fn with_symbol(mut self, sym: &str) -> Self {
        self.symbols.push(sym.to_string());
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({ "op": "subscribe", "args": self.symbols }).to_string()
    }
}
```

**One message per symbol** (Hyperliquid style — `build()` returns `Vec<String>`):

```rust
pub fn build(self) -> Vec<String> {
    self.symbols.iter()
        .map(|s| serde_json::json!({ "method": "subscribe", "subscription": { "type": "l2Book", "coin": s } }).to_string())
        .collect()
}
```

Use `ClientConfig::set_subscription_messages(vec)` at the call site when `build()` returns a `Vec<String>`.

### 4c. `msg_parser.rs` — message parser

Implements `MsgParserTrait`. Convert raw JSON text into `Vec<StreamMessage>`.

```rust
use crate::connection::MsgParserTrait;
use crate::types::ExchangeName;
use crate::types::orderbook::{GenericOrder, OrderbookAction, StreamMessage, OrderbookUpdate};
use anyhow::Result;

pub struct MyExchangeMessageParser;

impl MsgParserTrait<StreamMessage> for MyExchangeMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        // 1. Deserialise outer envelope
        // 2. Match on event type
        // 3. Build Vec<GenericOrder> from bids/asks
        // 4. Return StreamMessage::OrderbookUpdate(...)
        todo!()
    }

    /// Return Some("...") for a custom text ping. Return None for a bare WS ping.
    fn build_ping(&self) -> Option<String> {
        Some(r#"{"op":"ping"}"#.to_string())
    }

    fn is_pong(&self, text: &str) -> bool { text.contains("\"pong\"") }
    fn is_ping(&self, text: &str) -> bool { text.contains("\"ping\"") }
}
```

**`OrderbookAction` values:**

| Action | When to use |
|---|---|
| `Snapshot` | Message replaces the entire book |
| `Update` | Message updates individual price levels |
| `Delete` | Price level should be removed (size == 0) |

**`GenericOrder` fields:**

```rust
GenericOrder {
    price:     f64,
    qty:       f64,
    side:      String,   // "Bid" or "Ask"
    symbol:    String,
    timestamp: String,   // ts.to_rfc3339()
}
```

**Timestamp helpers:**

```rust
// From Unix milliseconds as u64:
let ts = Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now);

// From Unix milliseconds as a string:
use libs::time::parse_timestamp_ms;
let ts = parse_timestamp_ms(&event.timestamp_str);
```

### 4d. `client.rs` — connection wrapper

Thin wrapper around `ClientBase`. Most exchanges need nothing beyond forwarding:

```rust
use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
use crate::types::ExchangeName;
use crate::types::orderbook::StreamMessage;
use tokio::sync::mpsc::UnboundedSender;

pub struct MyExchangeClient {
    inner: ClientBase<MyExchangeMessageParser, StreamMessage>,
}

impl MyExchangeClient {
    pub fn new(
        config: ClientConfig,
        message_tx: UnboundedSender<StreamMessage>,
        system_control: SystemControl,
    ) -> Self {
        let inner = ClientBase::new(
            config,
            message_tx,
            system_control,
            MyExchangeMessageParser,
            ExchangeName::MyExchange,
            None,  // optional header builder
        );
        Self { inner }
    }
}

impl ClientTrait for MyExchangeClient {
    async fn run(&mut self) -> anyhow::Result<()> { self.inner.run().await }
}
```

`ClientBase` sends each string in `config.subscription_messages` as a separate WebSocket text frame on connect, so **multi-symbol** exchanges just need `set_subscription_messages(builder.build())` at the call site — no special splitting logic needed in the client.

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

---

## 5. Register in the connection factory

**`orderbook/src/exchanges/mod.rs`**

```rust
pub mod myexchange;
use crate::exchanges::myexchange::MyExchangeClient;

// Inside spawn_connection():
ExchangeName::MyExchange => {
    let mut c = MyExchangeClient::new(config, message_tx, system_control);
    tokio::spawn(async move { let _ = c.run().await })
}
```

---

## 6. Expose the public API

**`orderbook/src/lib.rs`**

```rust
pub use exchanges::myexchange::{MyExchangeClient, MyExchangeSubMsgBuilder};
```

---

## 7. Add server CLI + config support

**`libs/src/configs/`**

```rust
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MyExchangeConfig {
    pub enabled: bool,
    pub symbols: Vec<String>,
}
```

Add to the top-level `Config`:

```rust
pub myexchange: MyExchangeConfig,
```

**`zmq_server/src/bin/orderbook_server.rs`**

```rust
/// Comma-separated MyExchange symbols (overrides config file)
#[arg(long, env = "MYEXCHANGE_SYMBOLS", value_delimiter = ',')]
myexchange: Option<Vec<String>>,
```

```rust
if !myexchange_symbols.is_empty() {
    let refs: Vec<&str> = myexchange_symbols.iter().map(String::as_str).collect();
    system_config.with_exchange(
        ClientConfig::new(ExchangeName::MyExchange)
            .set_subscription_message(MyExchangeSubMsgBuilder::new().with_symbols(&refs).build()),
    );
}
```

**`configs/server.yaml`**

```yaml
myexchange:
  enabled: false
  symbols: ["BTC", "ETH"]
```

---

## Checklist

- [ ] `types/mod.rs` — `ExchangeName` variant + `to_str` + `from_str`
- [ ] `types/endpoints.rs` — WebSocket URL constant
- [ ] `connection/configs.rs` — URL match arm (+ ping_interval if non-default)
- [ ] `exchanges/myexchange/types.rs` — wire format structs
- [ ] `exchanges/myexchange/subscription.rs` — subscription builder
- [ ] `exchanges/myexchange/msg_parser.rs` — `MsgParserTrait` impl
- [ ] `exchanges/myexchange/client.rs` — `ClientTrait` impl
- [ ] `exchanges/myexchange/mod.rs` — re-exports
- [ ] `exchanges/mod.rs` — factory arm
- [ ] `orderbook/src/lib.rs` — public re-exports
- [ ] `libs/src/configs/` — config struct
- [ ] `zmq_server/src/bin/orderbook_server.rs` — CLI flag + exchange block
- [ ] `configs/server.yaml` — config section
- [ ] `cargo check --workspace` — fix any exhaustive match errors

---

## Reference: existing connectors

| Exchange | Snapshot type | Multi-symbol | Custom ping | Notes |
|---|---|---|---|---|
| Binance | Partial depth (incremental) | Single JSON array | None (WS ping) | `@depth20@100ms` stream |
| OKX | Snapshot + incremental | Single JSON array | `{"op":"ping"}` | |
| Polymarket | Snapshot + incremental diff | Per-asset set in one message | None | Token IDs, not symbols |
| Hyperliquid | Full snapshot every update | `Vec<String>` — one frame per coin | `{"method":"ping"}` | Use `set_subscription_messages` |
| Kalshi | Snapshot + signed delta | Single message | Server-side ping, client responds | RSA auth required |
