/// Binance WebSocket endpoints
#[derive(Debug, Clone, Copy)]
pub struct BinanceWs {
    pub public_stream: &'static str,
    pub private_stream: &'static str,
}

impl BinanceWs {
    pub fn new() -> Self {
        Self {
            public_stream: "wss://stream.binance.com:9443/ws",
            private_stream: "wss://stream.binance.com:9443/ws",
        }
    }
}
