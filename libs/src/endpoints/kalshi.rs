pub mod kalshi {
    /// Production REST base.
    pub const BASE_URL: &str = "https://api.elections.kalshi.com/trade-api/v2";
    /// Demo REST base.
    pub const DEMO_BASE_URL: &str = "https://demo-api.kalshi.co/trade-api/v2";

    /// Path prefix shared by every Kalshi REST endpoint (already part of `BASE_URL`).
    /// `KalshiCredentials::build_headers(method, path)` requires the *signed* path
    /// to include this prefix. Use `endpoints::sign_path()` to construct it.
    pub mod endpoints {
        pub const PATH_PREFIX: &str = "/trade-api/v2";

        // Portfolio / orders
        pub const PORTFOLIO_ORDERS: &str = "/portfolio/orders";
        pub const PORTFOLIO_ORDERS_BATCHED: &str = "/portfolio/orders/batched";
        /// Prefix — append `{order_id}` to cancel/get a single order.
        pub const PORTFOLIO_ORDER_BY_ID: &str = "/portfolio/orders/";
        /// Prefix — append `{ticker}` to cancel all orders on one market.
        pub const PORTFOLIO_ORDERS_BY_MARKET: &str = "/portfolio/orders/market/";
        /// Suffix appended after the order id to shrink an existing order.
        pub const PORTFOLIO_ORDER_DECREASE_SUFFIX: &str = "/decrease";

        // Heartbeat
        pub const EXCHANGE_HEARTBEAT: &str = "/exchange/heartbeat";

        /// Build the full sign-path for an endpoint (prefixes `/trade-api/v2`).
        pub fn sign_path(endpoint: &str) -> String {
            format!("{PATH_PREFIX}{endpoint}")
        }
    }

    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://api.elections.kalshi.com/trade-api/ws/v2";
        pub const DEMO_PUBLIC_STREAM: &str = "wss://demo-api.kalshi.co/trade-api/ws/v2";
        /// Path used when signing the WS upgrade request.
        pub const SIGN_PATH: &str = "/trade-api/ws/v2";
    }
}
