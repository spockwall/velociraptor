# Adding a New Exchange

This guide walks through every file you need to touch to add a new exchange connector. Follow the steps in order — the compiler will guide you if you miss anything because `ExchangeName` uses exhaustive match arms.

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
            // ...
            ExchangeName::MyExchange => "myexchange",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            // ...
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

## 3. Wire up the URL in ConnectionConfig

**`orderbook/src/connection/configs.rs`**

Add to the import:
```rust
use crate::types::endpoints::{binance, myexchange, okx, polymarket};
```

Add the match arm in `ConnectionConfig::new()`. Also set the `ping_interval` here if your exchange needs something other than the default 15s:

```rust
// Simple: same ping_interval for all exchanges
let ws_url = match name {
    // ...
    ExchangeName::MyExchange => myexchange::ws::PUBLIC_STREAM,
};

// Or: per-exchange ping_interval (e.g. Hyperliquid needs 45s)
let (ws_url, ping_interval) = match name {
    ExchangeName::Binance    => (binance::ws::PUBLIC_STREAM,    15u64),
    ExchangeName::MyExchange => (myexchange::ws::PUBLIC_STREAM, 45u64),
    // ...
};
Self { ws_url: ws_url.to_string(), ping_interval, .. }
```

---

## 4. Create the exchange module

Create the directory `orderbook/src/exchanges/myexchange/` with five files:

### 4a. `types.rs` — wire format structs

Deserialisation-only structs that match the exchange's raw JSON. Keep these private to the module.

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct MyLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Deserialize)]
pub struct MyBookEvent {
    pub symbol: String,
    pub bids: Vec<MyLevel>,
    pub asks: Vec<MyLevel>,
    pub timestamp: u64,   // or String — depends on exchange
}
```

### 4b. `subscription.rs` — subscription message builder

Build the JSON string(s) to send on connect. Follow the builder pattern:

```rust
pub struct MyExchangeSubMsgBuilder {
    symbols: Vec<String>,
}

impl MyExchangeSubMsgBuilder {
    pub fn new() -> Self { Self { symbols: vec![] } }

    pub fn with_symbol(mut self, sym: &str) -> Self {
        self.symbols.push(sym.to_string());
        self
    }

    pub fn build(self) -> String {
        // Single subscription object covering all symbols:
        serde_json::json!({ "op": "subscribe", "args": self.symbols }).to_string()

        // Or one message per symbol (newline-delimited) — see multi-symbol note below:
        // self.symbols.iter().map(|s| json!(...).to_string()).collect::<Vec<_>>().join("\n")
    }
}

impl Default for MyExchangeSubMsgBuilder {
    fn default() -> Self { Self::new() }
}
```

**Multi-symbol note:** If the exchange requires one subscription message per symbol (e.g. Hyperliquid), produce newline-delimited JSON in `build()` and split it in `connection.rs` — see the multi-symbol pattern below.

### 4c. `msg_parser.rs` — message parser

Implements `MessageParserTrait`. Convert raw JSON text into `Vec<OrderbookMessage>`.

```rust
use crate::connection::MessageParserTrait;
use crate::exchanges::myexchange::types::MyBookEvent;
use crate::types::ExchangeName;
use crate::types::orderbook::{GenericOrder, OrderbookAction, OrderbookMessage, OrderbookUpdate};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use tracing::error;

pub struct MyExchangeMessageParser {
    exchange_name: ExchangeName,
}

impl MyExchangeMessageParser {
    pub fn new() -> Self {
        Self { exchange_name: ExchangeName::MyExchange }
    }
}

impl Default for MyExchangeMessageParser {
    fn default() -> Self { Self::new() }
}

impl MessageParserTrait<OrderbookMessage> for MyExchangeMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<OrderbookMessage>> {
        // 1. Deserialise the outer envelope
        // 2. Match on event type / channel
        // 3. Build Vec<GenericOrder> from bids and asks
        // 4. Return OrderbookMessage::OrderbookUpdate(...)
        todo!()
    }

    // Optional overrides — all have sensible defaults in the trait:

    /// Return Some("...") if the exchange needs a custom text ping.
    /// The infrastructure wraps this in a WebSocket binary ping control frame.
    /// Return None to send a bare empty WebSocket ping.
    fn build_ping(&self) -> Option<String> {
        Some(r#"{"op":"ping"}"#.to_string())
    }

    /// Return true if the text message is an application-level pong.
    fn is_pong(&self, text: &str) -> bool {
        text.contains("\"pong\"")
    }

    /// Return true if the server sends application-level pings you must respond to.
    fn is_ping(&self, text: &str) -> bool {
        text.contains("\"ping\"")
    }
}
```

**`OrderbookAction` values:**

| Action | When to use |
|--------|------------|
| `Snapshot` | Message replaces the entire book (most common) |
| `Update`   | Message updates individual price levels |
| `Delete`   | Price level should be removed (size == 0) |

**Timestamp helpers:**

```rust
// From Unix milliseconds as u64:
let ts = Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now);

// From Unix milliseconds as a string (Polymarket style):
use libs::time::parse_timestamp_ms;
let ts = parse_timestamp_ms(&event.timestamp_str);
```

**`GenericOrder` fields:**

```rust
GenericOrder {
    price:     f64,
    qty:       f64,
    side:      String,   // "Bid" or "Ask"
    symbol:    String,   // exchange-native symbol key
    timestamp: String,   // ts.to_rfc3339()
}
```

### 4d. `connection.rs` — connection wrapper

Thin wrapper around `ConnectionBase`. Most exchanges need nothing beyond forwarding:

```rust
use crate::connection::{ConnectionConfig, ConnectionTrait, SystemControl, v1::ConnectionBase};
use crate::exchanges::myexchange::MyExchangeMessageParser;
use crate::types::ExchangeName;
use crate::types::orderbook::OrderbookMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

pub struct MyExchangeConnection {
    inner: ConnectionBase<MyExchangeMessageParser, OrderbookMessage>,
}

impl MyExchangeConnection {
    pub fn new(
        config: ConnectionConfig,
        message_tx: UnboundedSender<OrderbookMessage>,
        system_control: SystemControl,
    ) -> Self {
        let inner = ConnectionBase::new(
            config,
            message_tx,
            system_control,
            MyExchangeMessageParser::new(),
            ExchangeName::MyExchange,
            vec![],   // pre_subscription_messages  (sent before subscribe, e.g. auth)
            vec![],   // post_subscription_messages (sent after subscribe)
        );
        Self { inner }
    }
}

#[async_trait]
impl ConnectionTrait for MyExchangeConnection {
    async fn run(&mut self) -> Result<()> { self.inner.run().await }
    fn get_exchange_config(&self) -> &ConnectionConfig { self.inner.get_exchange_config() }
}
```

**Multi-symbol pattern** (one WS message per symbol):

```rust
impl MyExchangeConnection {
    pub fn new(config: ConnectionConfig, ...) -> Self {
        // Builder produced newline-delimited JSON — split here
        let mut lines: Vec<String> = config.subscription_message
            .split('\n')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let first = if lines.is_empty() { String::new() } else { lines.remove(0) };
        let rest  = lines;

        let config = config.set_subscription_message(first);

        let inner = ConnectionBase::new(config, message_tx, system_control,
            MyExchangeMessageParser::new(), ExchangeName::MyExchange,
            vec![],   // pre
            rest,     // post — ConnectionBase sends these as text frames after subscribe
        );
        Self { inner }
    }
}
```

### 4e. `mod.rs` — re-exports

```rust
pub mod connection;
pub mod msg_parser;
pub mod subscription;
pub mod types;

pub use connection::MyExchangeConnection;
pub(crate) use msg_parser::MyExchangeMessageParser;
pub use subscription::MyExchangeSubMsgBuilder;
```

---

## 5. Register in the connection factory

**`orderbook/src/exchanges/mod.rs`**

```rust
pub mod myexchange;

use crate::exchanges::myexchange::MyExchangeConnection;

// Inside ConnectionFactory::create_connection():
ExchangeName::MyExchange => Box::new(MyExchangeConnection::new(
    self.config.clone(),
    self.message_tx.clone(),
    system_control,
)),
```

---

## 6. Add dynamic channel support

**`orderbook/src/orderbook/system.rs`**

Add import at the top:
```rust
use crate::exchanges::myexchange::MyExchangeSubMsgBuilder;
```

Add arm in `build_connection_config()`:
```rust
ExchangeName::MyExchange => MyExchangeSubMsgBuilder::new()
    .with_symbol(&req.symbol)
    .build(),
```

This lets clients add symbols at runtime via the ZMQ ROUTER socket `add_channel` action.

---

## 7. Expose the public API

**`orderbook/src/lib.rs`**

```rust
pub use exchanges::myexchange::MyExchangeConnection;
pub use exchanges::myexchange::MyExchangeSubMsgBuilder;
```

---

## 8. Add server CLI + config support

**`orderbook/src/bin/orderbook_server.rs`**

Add a TOML config struct:
```rust
#[derive(Debug, Deserialize, Default)]
struct MyExchangeConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    symbols: Vec<String>,
}
```

Add to `TomlConfig`:
```rust
#[serde(default)]
myexchange: MyExchangeConfig,
```

Add a CLI flag in `Args`:
```rust
/// Comma-separated MyExchange symbols (overrides config file)
#[arg(long, env = "MYEXCHANGE_SYMBOLS", value_delimiter = ',')]
myexchange: Option<Vec<String>>,
```

Add to `run()`:
```rust
let myexchange_symbols: Vec<String> = cli_myexchange.unwrap_or_else(|| {
    if toml_cfg.myexchange.enabled { toml_cfg.myexchange.symbols.clone() } else { vec![] }
});

// ...include in empty-check...

if !myexchange_symbols.is_empty() {
    let refs: Vec<&str> = myexchange_symbols.iter().map(String::as_str).collect();
    config.with_exchange(
        ConnectionConfig::new(ExchangeName::MyExchange)
            .set_subscription_message(MyExchangeSubMsgBuilder::new().with_symbols(&refs).build()),
    );
}
```

**`config/server.toml`**

```toml
[myexchange]
enabled = false
symbols = ["BTC", "ETH"]
```

---

## Checklist

- [ ] `types/mod.rs` — `ExchangeName` variant + `to_str` + `from_str`
- [ ] `types/endpoints.rs` — WebSocket URL constant
- [ ] `connection/configs.rs` — URL match arm (+ ping_interval if non-default)
- [ ] `exchanges/myexchange/types.rs` — wire format structs
- [ ] `exchanges/myexchange/subscription.rs` — subscription builder
- [ ] `exchanges/myexchange/msg_parser.rs` — `MessageParserTrait` impl
- [ ] `exchanges/myexchange/connection.rs` — `ConnectionTrait` impl
- [ ] `exchanges/myexchange/mod.rs` — re-exports
- [ ] `exchanges/mod.rs` — factory arm
- [ ] `orderbook/system.rs` — `build_connection_config()` arm
- [ ] `lib.rs` — public re-exports
- [ ] `bin/orderbook_server.rs` — CLI flag + TOML struct + exchange block
- [ ] `config/server.toml` — config section
- [ ] `cargo build` — fix any exhaustive match errors

---

## Reference: existing connectors

| Exchange | Snapshot type | Multi-symbol | Custom ping | Timestamp type |
|----------|--------------|--------------|-------------|----------------|
| Binance | Partial depth (incremental) | Yes — one WS msg with array | None (WS ping) | String ms |
| OKX | Snapshot + incremental | Yes — one WS msg with array | `{"op":"ping"}` | String ms |
| Polymarket | Snapshot + incremental diff | No — one connection per asset set | None | String ms |
| Hyperliquid | Full snapshot every update | Yes — one msg per coin (newline-split) | `{"method":"ping"}` | u64 ms number |
