pub mod endpoints;
pub mod errors;
pub mod events;
pub mod orderbook;
pub use events::{OrderbookEvent, OrderbookSnapshot, PriceLevelTuple};

use core::fmt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExchangeName {
    Okx,
    Binance,
    Polymarket,
    Hyperliquid,
}

impl ExchangeName {
    pub fn to_str(&self) -> &'static str {
        match self {
            ExchangeName::Okx => "okx",
            ExchangeName::Binance => "binance",
            ExchangeName::Polymarket => "polymarket",
            ExchangeName::Hyperliquid => "hyperliquid",
        }
    }

    pub fn to_string(&self) -> String {
        self.to_str().to_string()
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "okx" => Some(ExchangeName::Okx),
            "binance" => Some(ExchangeName::Binance),
            "polymarket" => Some(ExchangeName::Polymarket),
            "hyperliquid" => Some(ExchangeName::Hyperliquid),
            _ => None,
        }
    }
}

impl fmt::Display for ExchangeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}
