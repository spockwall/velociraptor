pub mod binance {
    pub const BASE_URL: &str = "https://api.binance.com";

    pub mod ws {
        /// Binance USDT-margined futures public stream.
        pub const PUBLIC_STREAM: &str = "wss://fstream.binance.com/ws";
        /// Binance spot public combined stream.
        /// Combined endpoint wraps each frame as `{"stream":"<name>","data":{...}}`,
        /// which lets us recover the symbol for streams whose `data` payload
        /// omits it (e.g. `@depth20@100ms`).
        pub const SPOT_PUBLIC_STREAM: &str = "wss://stream.binance.com:9443/stream";
    }
}
