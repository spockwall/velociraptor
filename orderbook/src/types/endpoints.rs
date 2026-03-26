pub mod binance {
    pub const BASE_URL: &str = "https://api.binance.com";

    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://fstream.binance.com/ws";
    }
}

pub mod okx {
    pub const BASE_URL: &str = "https://www.okx.com";

    pub mod endpoints {

        // Account
        pub const ACCOUNT_INSTRUMENTS: &str = "/api/v5/account/instruments";
        pub const ACCOUNT_BALANCE: &str = "/api/v5/account/balance";
        pub const ACCOUNT_POSITIONS: &str = "/api/v5/account/positions";
        pub const ACCOUNT_POSITIONS_HISTORY: &str = "/api/v5/account/positions-history";
        pub const ACCOUNT_POSITION_RISK: &str = "/api/v5/account/account-position-risk";
        pub const ACCOUNT_BILLS: &str = "/api/v5/account/bills";
        pub const ACCOUNT_BILLS_ARCHIVE: &str = "/api/v5/account/bills-archive";
        pub const ACCOUNT_BILLS_HISTORY_ARCHIVE: &str = "/api/v5/account/bills-history-archive";
        pub const ACCOUNT_CONFIG: &str = "/api/v5/account/config";
        pub const ACCOUNT_POSITION_MODE: &str = "/api/v5/account/set-position-mode";
        pub const ACCOUNT_SET_LEVERAGE: &str = "/api/v5/account/set-leverage";
        pub const ACCOUNT_MAX_TRADE_SIZE: &str = "/api/v5/account/max-size";
        pub const ACCOUNT_MAX_AVAIL_SIZE: &str = "/api/v5/account/max-avail-size";
        pub const ACCOUNT_LEVERAGE_INFO: &str = "/api/v5/account/leverage-info";
        pub const ACCOUNT_ADJUST_LEVERAGE_INFO: &str = "/api/v5/account/adjust-leverage-info";
        pub const ACCOUNT_MAX_LOAN: &str = "/api/v5/account/max-loan";
        pub const ACCOUNT_FEE_RATES: &str = "/api/v5/account/trade-fee";
        pub const ACCOUNT_INTEREST_ACCRUED: &str = "/api/v5/account/interest-accrued";
        pub const ACCOUNT_INTEREST_RATE: &str = "/api/v5/account/interest-rate";
        pub const ACCOUNT_SET_GREEKS: &str = "/api/v5/account/set-greeks";
        pub const ACCOUNT_SET_ISOLATED_MODE: &str = "/api/v5/account/set-isolated-mode";
        pub const ACCOUNT_MAX_WITHDRAWAL: &str = "/api/v5/account/max-withdrawal";
        pub const ACCOUNT_INTEREST_LIMITS: &str = "/api/v5/account/interest-limits";
        pub const ACCOUNT_SET_AUTO_LOAN: &str = "/api/v5/account/set-auto-loan";

        // Asset
        pub const ASSET_CURRENCIES: &str = "/api/v5/asset/currencies";
        pub const ASSET_BALANCES: &str = "/api/v5/asset/balances";
        pub const ASSET_VALUATION: &str = "/api/v5/asset/asset-valuation";
        pub const ASSET_TRANSFER: &str = "/api/v5/asset/transfer"; // funds transfer
        pub const ASSET_TRANSFER_STATE: &str = "/api/v5/asset/transfer-state";
        pub const ASSET_BILLS: &str = "/api/v5/asset/bills";
        pub const ASSET_DEPOSIT_ADDRESS: &str = "/api/v5/asset/deposit-address";
        pub const ASSET_DEPOSIT_HISTORY: &str = "/api/v5/asset/deposit-history";
        pub const ASSET_WITHDRAWAL_COIN: &str = "/api/v5/asset/withdrawal";
        pub const ASSET_CANCEL_WITHDRAWAL: &str = "/api/v5/asset/cancel-withdrawal";
        pub const ASSET_WITHDRAWAL_HISTORY: &str = "/api/v5/asset/withdrawal-history";
        pub const ASSET_DEPOSIT_WITHDRAW_STATUS: &str = "/api/v5/asset/deposit-withdraw-status";
        pub const ASSET_EXCHANGE_LIST: &str = "/api/v5/asset/exchange-list";
        pub const ASSET_MONTHLY_STATEMENT: &str = "/api/v5/asset/monthly-statement";
        pub const ASSET_CONVERT_CURRENCIES: &str = "/api/v5/asset/convert/currencies";
        pub const ASSET_CONVERT_HISTORY: &str = "/api/v5/asset/convert/history";

        // Market
        pub const MARKET_CANDLES: &str = "/api/v5/market/candles";
        pub const MARKET_ORDERBOOK: &str = "/api/v5/market/books";
        pub const MARKET_TICKERS: &str = "/api/v5/market/tickers";
        pub const MARKET_INSTRUMENTS: &str = "/api/v5/public/instruments";
        pub const MARKET_FUNDING_RATE: &str = "/api/v5/public/funding-rate";
        pub const MARKET_FUNDING_RATE_HISTORY: &str = "/api/v5/public/funding-rate-history";

        // Trade
        pub const TRADE_ORDER: &str = "/api/v5/trade/order";
        pub const TRADE_ORDER_LIST: &str = "/api/v5/trade/orders-pending";
        pub const TRADE_BATCH_ORDERS: &str = "/api/v5/trade/batch-orders";
        pub const TRADE_CANCEL_ORDER: &str = "/api/v5/trade/cancel-order";
        pub const TRADE_CANCEL_BATCH_ORDERS: &str = "/api/v5/trade/cancel-batch-orders";
        pub const TRADE_AMEND_ORDER: &str = "/api/v5/trade/amend-order";
        pub const TRADE_AMEND_BATCH_ORDER: &str = "/api/v5/trade/amend-batch-orders";
        pub const TRADE_CLOSE_POSITION: &str = "/api/v5/trade/close-position";
        pub const TRADE_ORDERS_PENDING: &str = "/api/v5/trade/orders-pending";
        pub const TRADE_ORDERS_HISTORY: &str = "/api/v5/trade/orders-history";
        pub const TRADE_ORDERS_HISTORY_ARCHIVE: &str = "/api/v5/trade/orders-history-archive";
        pub const TRADE_ORDER_FILLS: &str = "/api/v5/trade/fills";
        pub const TRADE_ORDERS_FILLS_HISTORY: &str = "/api/v5/trade/fills-history";
        pub const TRADE_ACCOUNT_RATE_LIMIT: &str = "/api/v5/trade/account-rate-limit";
    }

    pub mod ws {
        pub const PUBLIC_STREAM: &str = "wss://ws.okx.com:8443/ws/v5/public";
        pub const PRIVATE_STREAM: &str = "wss://ws.okx.com:8443/ws/v5/private";
    }
}
