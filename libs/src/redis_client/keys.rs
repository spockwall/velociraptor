//! Redis key schema. Keep all key construction here so the layout is
//! defined in exactly one place.
//!
pub struct RedisKey;
impl RedisKey {
    pub fn orderbook(exchange: &str, symbol: &str) -> String {
        format!("ob:{exchange}:{symbol}")
    }
    pub fn bba(exchange: &str, symbol: &str) -> String {
        format!("bba:{exchange}:{symbol}")
    }
    /// Capped list of recent orderbook snapshots: `snapshots:{exchange}:{symbol}`
    pub fn snapshots(exchange: &str, symbol: &str) -> String {
        format!("snapshots:{exchange}:{symbol}")
    }
    /// Capped list of recent last-trade events: `trades:{exchange}:{symbol}`
    pub fn trades(exchange: &str, symbol: &str) -> String {
        format!("trades:{exchange}:{symbol}")
    }

    pub fn position(exchange: &str, symbol: &str) -> String {
        format!("position:{exchange}:{symbol}")
    }

    pub fn balance(exchange: &str, asset: &str) -> String {
        format!("balance:{exchange}:{asset}")
    }

    pub fn orders_open(exchange: &str) -> String {
        format!("orders:open:{exchange}")
    }

    /// Per-asset label hash for Polymarket: `polymarket:label:{asset_id}`
    /// Stored as a Redis hash with fields: base_slug, full_slug, side, window_start
    pub fn polymarket_label(asset_id: &str) -> String {
        format!("polymarket:label:{asset_id}")
    }

    /// Set membership of all currently-labeled Polymarket asset_ids.
    pub const POLYMARKET_LABEL_INDEX: &'static str = "polymarket:label:index";

    /// Set of asset_ids belonging to the currently-active window for `base_slug`.
    /// Used to evict prior-window labels when a new window starts.
    pub fn polymarket_base_slug_assets(base_slug: &str) -> String {
        format!("polymarket:base:{base_slug}:assets")
    }

    /// Per-ticker label hash for Kalshi: `kalshi:label:{ticker}`
    /// Stored as a Redis hash with fields: series, ticker, window_start, window_close, interval_secs
    pub fn kalshi_label(ticker: &str) -> String {
        format!("kalshi:label:{ticker}")
    }

    /// Set membership of all currently-labeled Kalshi market tickers.
    pub const KALSHI_LABEL_INDEX: &'static str = "kalshi:label:index";

    /// Set of tickers belonging to the currently-active window for `series`.
    /// Used to evict prior-window labels when a new window starts.
    pub fn kalshi_series_tickers(series: &str) -> String {
        format!("kalshi:series:{series}:tickers")
    }
}

pub struct Events;
impl Events {
    pub const FILLS: &'static str = "events:fills";
    pub const ORDERS: &'static str = "events:orders";
    pub const LOG: &'static str = "events:log";
}

pub struct Engine;
impl Engine {
    pub const STATUS: &'static str = "engine:status";
    pub const PARAMS: &'static str = "engine:params";
}

pub struct Risk;
impl Risk {
    pub const CONFIG: &'static str = "risk:config";
    pub const KILL_SWITCH: &'static str = "risk:kill_switch";
}

/// Redis pub/sub channel used for config hot-reload notifications.
pub const CONFIG_UPDATES_CHANNEL: &str = crate::constants::REDIS_CONFIG_UPDATES_CHANNEL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_formats() {
        assert_eq!(
            RedisKey::orderbook("polymarket", "TRUMP-2028"),
            "ob:polymarket:TRUMP-2028"
        );
        assert_eq!(RedisKey::bba("kalshi", "PRES-2028"), "bba:kalshi:PRES-2028");
        assert_eq!(
            RedisKey::orders_open("polymarket"),
            "orders:open:polymarket"
        );
    }
}
