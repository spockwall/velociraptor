pub mod polymarket {
    /// Production CLOB v2 host. Cutover from v1 was 2026-04-28.
    pub const BASE_URL: &str = "https://clob.polymarket.com";
    /// Testnet / preprod CLOB v2 host.
    pub const TESTNET_BASE_URL: &str = "https://clob-v2.polymarket.com";
    


    pub mod endpoints {
        // Order endpoints
        pub const POST_ORDER: &str = "/order";
        pub const POST_ORDERS: &str = "/orders";
        pub const CANCEL_ORDER: &str = "/order";
        pub const CANCEL_ORDERS: &str = "/orders";
        pub const CANCEL_ALL: &str = "/cancel-all";
        pub const CANCEL_MARKET_ORDERS: &str = "/cancel-market-orders";

        // Data / read
        /// Prefix — append `{order_id}` to fetch a single order.
        pub const GET_ORDER: &str = "/data/order/";
        pub const GET_OPEN_ORDERS: &str = "/data/orders";
        pub const GET_TRADES: &str = "/data/trades";

        // Health / time
        pub const HEARTBEAT: &str = "/v1/heartbeats";
        pub const TIME: &str = "/time";
        pub const OK: &str = "/ok";
    }

    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
        pub const USER_STREAM: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/user";
    }
}
