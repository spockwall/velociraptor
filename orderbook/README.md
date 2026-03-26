# Orderbook - Real-time Cryptocurrency Orderbook Library

A high-performance Rust library for real-time cryptocurrency orderbook data streaming, storage, and replay from major exchanges.

## Features

🚀 **Real-time Streaming**

- WebSocket connections to major exchanges (OKX, BitMEX, BitMart)
- Automatic reconnection and error handling
- Live orderbook state management

📊 **Data Storage & Replay**

- Efficient binary storage with compression (LZ4, Zstd)
- Multiple serialization formats (JSON, Cap'n Proto*)
- High-performance data replay for backtesting

🌐 **WebSocket Server**

- Stream live orderbook data to clients
- Replay historical data via WebSocket
- Real-time monitoring and statistics

⚡ **Performance**

- Zero-copy deserialization where possible
- Concurrent processing with Tokio
- Optimized for high-frequency data

\* *Cap'n Proto support is optional - compile with `--features capnp`*

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
orderbook = { path = "path/to/orderbook" }

# Optional: Enable Cap'n Proto serialization
orderbook = { path = "path/to/orderbook", features = ["capnp"] }
```

### Basic Usage

```rust
use krex::connectors::{ConnectorConfig, SystemControl};
use krex::exchanges::ExchangeName;
use krex::exchanges::okx::OkxSubMsgBuilder;
use orderbook::api::{OrderbookSystem, OrderbookSystemConfig};
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("orderbook=info")
        .init();

    // Build system configuration
    let mut config = OrderbookSystemConfig::new();
    config.with_exchange(
        ConnectorConfig::new(ExchangeName::Okx).set_subscription_message(
            OkxSubMsgBuilder::new()
                .with_orderbook_channel("BTC-USDT", "SWAP")
                .build(),
        ),
    );
    config.set_stats_interval(Duration::from_secs(5));

    // Validate configuration
    config.validate()?;

    // Create and run the orderbook system
    let system_control = SystemControl::new();
    let system = OrderbookSystem::new(config, system_control.clone())?;

    // Access orderbooks for real-time processing
    if let Some(orderbooks) = system.get_orderbooks() {
        info!("Orderbook system started with live orderbook tracking");
        
        // Access specific orderbook
        let key = "okx:BTC-USDT";
        if let Some(orderbook) = orderbooks.get(key) {
            if let (Some((bid_price, _)), Some((ask_price, _))) = 
                (orderbook.best_bid(), orderbook.best_ask()) {
                info!("BTC-USDT: Bid={:.2} Ask={:.2} Spread={:.2}", 
                    bid_price, ask_price, ask_price - bid_price);
            }
        }
    }

    // Run the system (blocks until shutdown)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down");
            system_control.shutdown();
        }
        result = system.run(true) => {
            if let Err(e) = result {
                eprintln!("System error: {}", e);
            }
        }
    }

    Ok(())
}
```

## Supported Exchanges

| Exchange | Spot Markets | Futures | WebSocket |
|----------|-------------|---------|------------|
| OKX      | ✅          | ✅       | ✅         | 
| BitMEX   | ✅          | ✅       | ✅         |
| BitMart  | ✅          | ✅       | ✅         | 

## Storage Formats

### Serialization

- **JSON**: Human-readable, debugging-friendly
- **Cap'n Proto**: Binary format, high performance (optional feature)

### Compression

- **None**: No compression
- **LZ4**: Fast compression/decompression
- **Zstd**: High compression ratio with configurable levels

```rust
use orderbook::api::OrderbookSystemConfig;
use orderbook::storage::types::StorageConfig;

let mut system_config = OrderbookSystemConfig::new();

// Configure storage with builder pattern
system_config.with_storage(|storage| {
    storage
        .set_path("./hot_storage")
        .set_buffer_size(100_000)
        .set_rotation_size_bytes(1_024 * 1_024 * 1_024) // 1GB
        .set_flush_interval(5)
        .use_json_format()
        .use_zstd_compression(3)
});

// Alternative: Direct StorageConfig creation
let storage_config = StorageConfig::new()
    .set_path("./data")
    .set_buffer_size(50_000)
    .set_rotation_size_bytes(500 * 1_024 * 1_024) // 500MB
    .set_flush_interval(10)
    .use_capnp_format()
    .use_lz4_compression()
    .build();
```

## Data Replay

Replay historical orderbook data for backtesting or analysis:

```rust
use orderbook::{StorageReader, OrderbookReplayer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reader = StorageReader::from_file("./data/OKX_BTC-USDT_20241215.bin")?;
    let mut replayer = OrderbookReplayer::new(reader);
    
    while let Some(message) = replayer.next_message().await? {
        println!("Timestamp: {}, Type: {}", 
            message.timestamp(), message.message_type());
        
        // Process historical message
        if let Some(update) = message.as_orderbook_update() {
            // Reconstruct orderbook state
            // Run trading strategies
            // Calculate metrics
        }
    }
    
    Ok(())
}
```

## WebSocket Server

The orderbook crate includes a powerful WebSocket server for streaming live and historical data:

### Basic Server Setup

```rust
use orderbook::api::websocket_server::OrderbookReplayServer;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt().init();

    // Create and start the WebSocket server
    let server = OrderbookReplayServer::new();
    let addr = "127.0.0.1:8080";
    
    info!("Starting WebSocket server on ws://{}", addr);
    server.start(addr).await?;
    
    Ok(())
}
```

### Client Connection Examples

#### JavaScript/Web Browser

```javascript
// Connect to live orderbook data
const ws = new WebSocket('ws://localhost:8080/live/OKX/BTC-USDT');
ws.onmessage = (event) => {
    const data = JSON.parse(event.data);
    console.log('Orderbook update:', data);
    
    // Process orderbook data
    if (data.type === 'orderbook_update') {
        const { symbol, bids, asks, timestamp } = data.data;
        console.log(`${symbol}: ${bids.length} bids, ${asks.length} asks`);
    }
};

// Connect to historical replay
const replayWs = new WebSocket('ws://localhost:8080/replay/path/to/data.bin');
replayWs.onmessage = (event) => {
    const message = JSON.parse(event.data);
    console.log('Historical message:', message);
};
```

#### Python Client

```python
import asyncio
import websockets
import json

async def connect_to_orderbook():
    uri = "ws://localhost:8080/live/OKX/BTC-USDT"
    async with websockets.connect(uri) as websocket:
        async for message in websocket:
            data = json.loads(message)
            print(f"Received: {data}")

asyncio.run(connect_to_orderbook())
```

### Server Features

- **Live Data Streaming**: Real-time orderbook updates from connected exchanges
- **Historical Replay**: Stream stored historical data at configurable speeds
- **Multiple Clients**: Support for concurrent WebSocket connections
- **Message Filtering**: Clients can subscribe to specific exchanges/symbols
- **Automatic Reconnection**: Built-in reconnection handling for clients
- **JSON Format**: All messages use JSON for easy integration

### WebSocket Endpoints

| Endpoint | Description | Example |
|----------|-------------|---------|
| `/live/{exchange}/{symbol}` | Live orderbook data | `/live/OKX/BTC-USDT` |
| `/replay/{file_path}` | Historical data replay | `/replay/data/20241215.bin` |
| `/status` | Server status and statistics | `/status` |

## Configuration

### Environment Variables

- Use different credential format to the one in mmengine, will be modified in the future

```bash
# Exchange API credentials (if needed)
export OKX_API_KEY="your_api_key"
export OKX_SECRET_KEY="your_secret"
export OKX_PASSPHRASE="your_passphrase"

export BITMEX_API_KEY="your_api_key"
export BITMEX_SECRET_KEY="your_secret"
```

### Storage Configuration

```rust
use orderbook::storage::types::StorageConfig;

let config = StorageConfig::new()
    .set_path("./hot_storage")
    .set_buffer_size(100_000)
    .set_rotation_size_bytes(1_024 * 1_024 * 1_024) // 1GB
    .set_flush_interval(5)
    .use_json_format()
    .use_lz4_compression()
    .build();
```

### Storage Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `hot_storage_path` | Base directory for stored files | `./hot_storage` |
| `channel_buffer_size` | Internal buffer size for message queuing | `100,000` |
| `rotation_size_bytes` | File size limit before rotation | `1GB` |
| `flush_interval_secs` | Interval between disk flushes | `5 seconds` |
| `serialization_format` | Data format (Json/CapnProto) | `CapnProto` (if feature enabled) |
| `compression_format` | Compression algorithm (None/Lz4/Zstd) | `Lz4` |

## Binary Tools

The orderbook crate provides several binary tools for common tasks:

### OKX Orderbook Recorder

High-performance orderbook data recording with custom compression:

```bash
cargo run --bin okx_orderbook_record
```

Records orderbook snapshots to compressed binary files with:

- 500ms tick interval (configurable)
- Zstd compression (level 9)
- Multiple symbol support (BTC-USDT, ETH-USDT, ADA-USDT, XRP-USDT, DOGE-USDT)
- Automatic file rotation by date
- Binary format optimized for storage efficiency

### File Management Tools

```bash
# Compact and optimize stored files
cargo run --bin compact_files

# Generate exchange login messages
cargo run --bin exchange_login_message
```

## Examples

The `examples/` directory contains complete working examples:

- **`stream_orderbook_instance.rs`** - Basic orderbook streaming with OrderbookSystem
- **`websocket_replayer_server.rs`** - WebSocket server for data replay
- **`websocket_replayer_client.rs`** - WebSocket client for connecting to replay server
- **`api_usage.rs`** - Comprehensive API usage examples with multiple scenarios
- **`replay_example.rs`** - Historical data processing and replay
- **`stream_connector_script.rs`** - Direct connector usage (legacy)
- **`read_stream_connector_script.rs`** - Reading from stored data

Run an example:

```bash
# Basic streaming example
cargo run --example stream_orderbook_instance

# WebSocket replay server
cargo run --example websocket_replayer_server

# Comprehensive API examples
cargo run --example api_usage
```

## Building

### Standard build (JSON only)

```bash
cargo build --release
```

### With Cap'n Proto support

```bash
cargo build --release --features capnp
```

### Without default features

```bash
cargo build --release --no-default-features
```

## Testing

Run the test suite:

```bash
# All tests
cargo test

# Integration tests with real exchanges (requires network)
cargo test --features integration_tests

# Cap'n Proto specific tests
cargo test --features capnp
```

## Performance

### Benchmarks (Intel i7-12700K, 32GB RAM)

| Metric | Value | Notes |
|--------|-------|-------|
| **Message Throughput** | 150,000+ msg/s | Single exchange, multiple symbols |
| **Storage Write Speed** | 80MB/s (compressed) | Zstd level 3 compression |
| **Memory Usage** | 50-200MB | Depends on orderbook depth and symbols |
| **Message Latency** | <1ms | End-to-end processing time |
| **WebSocket Connections** | 1000+ concurrent | Multiple clients supported |
| **File Rotation Time** | <100ms | 1GB file rotation overhead |
| **Compression Ratio** | 85-95% | Zstd compression efficiency |

### Performance Optimization Tips

1. **Buffer Sizing**: Increase `channel_buffer_size` for high-throughput scenarios
2. **Compression**: Use LZ4 for speed, Zstd for space efficiency
3. **File Rotation**: Adjust `rotation_size_bytes` based on disk I/O capacity
4. **Memory Management**: Monitor orderbook depth to control memory usage
5. **Concurrent Processing**: Use multiple system instances for different exchanges

### Resource Requirements

- **CPU**: 2+ cores recommended for multi-exchange setups
- **RAM**: 4GB minimum, 8GB+ recommended for production
- **Storage**: SSD recommended for optimal write performance
- **Network**: Stable connection with <100ms latency to exchanges

## Architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│                        OrderbookSystem                         │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐            │
│  │   OKX       │  │   BitMEX    │  │   BitMart   │            │
│  │ Connector   │  │ Connector   │  │ Connector   │            │
│  └─────────────┘  └─────────────┘  └─────────────┘            │
│         │               │               │                     │
│         └───────────────┼───────────────┘                     │
│                         ▼                                     │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Message Router                             │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │   │
│  │  │ Orderbook   │  │   Storage   │  │  WebSocket  │    │   │
│  │  │  Manager    │  │   Writer    │  │   Server    │    │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘    │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    External Interfaces                         │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐            │
│  │   Live      │  │ Historical  │  │   Binary    │            │
│  │ Orderbooks  │  │   Replay    │  │   Storage   │            │
│  │ (DashMap)   │  │ (WebSocket) │  │ (Files)     │            │
│  └─────────────┘  └─────────────┘  └─────────────┘            │
└─────────────────────────────────────────────────────────────────┘
```

### Component Overview

| Component | Purpose | Technology |
|-----------|---------|------------|
| **Exchange Connectors** | WebSocket connections to exchanges | tokio-tungstenite |
| **Message Router** | Distribute messages to components | Crossbeam channels |
| **Orderbook Manager** | Maintain real-time orderbook state | DashMap + BTreeMap |
| **Storage Writer** | Persist data to disk | Async I/O + Compression |
| **WebSocket Server** | Stream data to clients | tokio-tungstenite |
| **System Control** | Graceful shutdown & coordination | Atomic flags |

### Data Flow

1. **Ingestion**: Exchange connectors receive WebSocket messages
2. **Routing**: Messages are distributed to orderbook manager and storage
3. **Processing**: Orderbook state is updated in real-time
4. **Storage**: Messages are compressed and written to disk
5. **Streaming**: WebSocket server provides live and historical data
6. **Control**: System control handles graceful shutdown and coordination

## Contributing

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass: `cargo test`
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Changelog

### v0.1.0

- **Initial release**
- Support for OKX, BitMEX, BitMart exchanges
- JSON and Cap'n Proto serialization
- WebSocket server and replay functionality
- Comprehensive storage and compression options
- OrderbookSystem API for unified exchange management
- High-performance binary recording tools
- Real-time orderbook state management
- Multi-client WebSocket streaming
- Advanced storage configuration with builder pattern

## Related Projects

- **[krex](../krex/)** - Exchange connector library providing WebSocket and REST APIs
- **[mmengine](../mmengine/)** - Market making engine built on top of orderbook data
- **[demo](../demo/)** - Example applications and demos

## Support

For questions, issues, or contributions:

- Create an issue in the repository
- Check the examples directory for usage patterns
- Review the API documentation in the source code
