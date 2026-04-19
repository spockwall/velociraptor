# Adding a New Exchange

This guide walks through every file you need to touch to add a new exchange
connector. Follow the steps in order — the compiler will catch any missed
exhaustive match arms.

---

## 1. Register the exchange name

**`libs/src/protocol/mod.rs`**

Add a variant to `ExchangeName` and update both conversion methods:

```rust
pub enum ExchangeName {
    Okx,
    Binance,
    Polymarket,
    Hyperliquid,
    Kalshi,
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

## 3. Wire up the URL in `ClientConfig`

**`orderbook/src/connection/configs.rs`**

Add the match arm in `ClientConfig::new()`. Also set `ping_interval` here if
your exchange needs something other than the default:

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

Deserialisation-only structs matching the exchange's raw JSON. Keep these
private to the module.

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
        .map(|s| serde_json::json!({
            "method": "subscribe",
            "subscription": { "type": "l2Book", "coin": s }
        }).to_string())
        .collect()
}
```

Use `ClientConfig::set_subscription_messages(vec)` at the call site when
`build()` returns a `Vec<String>`.

### 4c. `msg_parser.rs` — message parser

Implements `MsgParserTrait`. Convert raw JSON text into `Vec<StreamMessage>`.

```rust
use crate::connection::MsgParserTrait;
use crate::types::orderbook::{GenericOrder, OrderbookAction, StreamMessage, OrderbookUpdate};
use anyhow::Result;
use libs::protocol::ExchangeName;

pub struct MyExchangeMessageParser;

impl MsgParserTrait<StreamMessage> for MyExchangeMessageParser {
    fn parse_message(&self, text: &str) -> Result<Vec<StreamMessage>> {
        // 1. Deserialise outer envelope
        // 2. Match on event type
        // 3. Build Vec<GenericOrder> from bids/asks
        // 4. Return StreamMessage::OrderbookUpdate(...)
        todo!()
    }

    /// Return Some("...") for a custom text ping. Return None to use bare WS ping.
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
| `Update` | Message updates individual price levels (qty = new total) |
| `Delete` | Price level should be removed (qty = 0) |

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
// From Unix milliseconds (u64):
let ts = Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now);

// From Unix milliseconds (string):
use libs::time::parse_timestamp_ms;
let ts = parse_timestamp_ms(&event.timestamp_str);
```

**Emit one `StreamMessage` per price-level change.** Do not group multiple
levels with different actions into one message — mixed Update/Delete batches
cause silent data loss. See `polymarket/msg_parser.rs` for the correct pattern.

### 4d. `client.rs` — connection wrapper

Thin wrapper around `ClientBase`:

```rust
use crate::connection::{ClientConfig, ClientTrait, SystemControl, client::ClientBase};
use crate::types::orderbook::StreamMessage;
use libs::protocol::ExchangeName;
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
            None,  // optional header builder (used by Kalshi for RSA auth)
        );
        Self { inner }
    }
}

impl ClientTrait for MyExchangeClient {
    async fn run(&mut self) -> anyhow::Result<()> { self.inner.run().await }
}
```

`ClientBase` sends each string in `config.subscription_messages` as a separate
WebSocket text frame on connect, so multi-symbol exchanges using
`set_subscription_messages(vec)` need no special splitting logic here.

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

## 7. Add config + server wiring

**`libs/src/configs/`** — add a config struct:

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

**`zmq_server/src/setup.rs`** — add a wiring helper (follow the pattern of
`add_binance` / `add_okx`):

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

**`zmq_server/src/bin/orderbook_server.rs`** — call the helper in `run()`:

```rust
let has_static = [
    cfg.binance.enabled     && add_binance(&mut system_cfg, &cfg.binance.symbols),
    cfg.okx.enabled         && add_okx(&mut system_cfg, &cfg.okx.symbols),
    cfg.hyperliquid.enabled && add_hyperliquid(&mut system_cfg, &cfg.hyperliquid.coins),
    cfg.myexchange.enabled  && add_myexchange(&mut system_cfg, &cfg.myexchange.symbols), // ADD
].iter().any(|&v| v);
```

**`configs/server.yaml`** — add the config section:

```yaml
myexchange:
  enabled: false
  symbols: ["BTC", "ETH"]
```

---

## Checklist

- [ ] `libs/src/protocol/mod.rs` — `ExchangeName` variant + `to_str` + `from_str`
- [ ] `orderbook/src/types/endpoints.rs` — WebSocket URL constant
- [ ] `orderbook/src/connection/configs.rs` — URL match arm (+ ping_interval if non-default)
- [ ] `orderbook/src/exchanges/myexchange/types.rs` — wire format structs
- [ ] `orderbook/src/exchanges/myexchange/subscription.rs` — subscription builder
- [ ] `orderbook/src/exchanges/myexchange/msg_parser.rs` — `MsgParserTrait` impl
- [ ] `orderbook/src/exchanges/myexchange/client.rs` — `ClientTrait` impl
- [ ] `orderbook/src/exchanges/myexchange/mod.rs` — re-exports
- [ ] `orderbook/src/exchanges/mod.rs` — factory match arm
- [ ] `orderbook/src/lib.rs` — public re-exports
- [ ] `libs/src/configs/` — config struct + add to `Config`
- [ ] `zmq_server/src/setup.rs` — `add_myexchange` helper
- [ ] `zmq_server/src/bin/orderbook_server.rs` — call helper in `has_static` array
- [ ] `configs/server.yaml` — config section
- [ ] `cargo check --workspace` — fix any exhaustive match errors

---

## Reference: existing connectors

| Exchange | Snapshot type | Multi-symbol | Custom ping | Notes |
|---|---|---|---|---|
| Binance | Partial depth (incremental) | Single JSON array | None (WS ping) | `@depth20@100ms` stream |
| OKX | Snapshot + incremental | Single JSON array | `{"op":"ping"}` | |
| Polymarket | Full snapshot + per-level price-change diffs | Per-asset in one message | `"PING"` text frame | Token IDs as symbols; one `StreamMessage` per price change |
| Hyperliquid | Full snapshot every update | `Vec<String>` (one frame per coin) | `{"method":"ping"}` | Use `set_subscription_messages` |
| Kalshi | Snapshot + signed dollar delta | Single message | Server-initiated (client pongs) | RSA-PSS auth on WS upgrade; ticker resolved via REST |

## Key design rules

- **One `StreamMessage` per price-level change.** Never batch levels with
  different actions — see `polymarket/msg_parser.rs`.
- **`ExchangeName` lives in `libs::protocol`**, not in the `orderbook` crate.
- **Wiring helpers live in `zmq_server/src/setup.rs`**, not in the binary.
  The binary calls helpers; helpers call `cfg.with_exchange(...)`.
- **Rolling-window schedulers** (Polymarket, Kalshi) live under
  `orderbook/src/exchanges/{exchange}/scheduler.rs` and expose
  `run_rolling_scheduler` + `WindowTask`. The binary calls
  `spawn_{exchange}_schedulers` from `setup.rs`.
