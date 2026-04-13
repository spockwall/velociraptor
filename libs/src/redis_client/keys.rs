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

    pub fn position(exchange: &str, symbol: &str) -> String {
        format!("position:{exchange}:{symbol}")
    }

    pub fn balance(exchange: &str, asset: &str) -> String {
        format!("balance:{exchange}:{asset}")
    }

    pub fn orders_open(exchange: &str) -> String {
        format!("orders:open:{exchange}")
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
