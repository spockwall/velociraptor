pub mod coinbase {
    pub mod ws {
        /// Public Coinbase Exchange market-data feed.
        pub const PUBLIC_STREAM: &str = "wss://ws-feed.exchange.coinbase.com";

        /// Public Coinbase Exchange sandbox market-data feed.
        pub const SANDBOX_PUBLIC_STREAM: &str =
            "wss://ws-feed-public.sandbox.exchange.coinbase.com";
    }
}
