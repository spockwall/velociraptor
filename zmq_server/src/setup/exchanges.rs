//! Static-exchange registration.
//!
//! These exchanges' symbols (e.g. `btcusdt`, `BTC-USDT`, `BTC`) are stable
//! over time, so we register them once at startup into the main
//! `StreamSystemConfig` — no scheduler / window lifecycle is involved.
//! Their orderbook data flows through the main `StreamEngine` and lands in
//! Redis via the generic [`super::attach_redis`] hook keyed by `snap.symbol`.

use libs::protocol::ExchangeName;
use orderbook::connection::ClientConfig;
use orderbook::exchanges::binance::BinanceSubMsgBuilder;
use orderbook::exchanges::hyperliquid::HyperliquidSubMsgBuilder;
use orderbook::exchanges::okx::OkxSubMsgBuilder;
use orderbook::StreamSystemConfig;
use tracing::info;

/// Register Binance (USDT-M futures) symbols into `cfg`. Returns `true` if any were added.
pub fn add_binance(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Binance).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&refs)
                .with_trade_channel(&refs)
                .build(),
        ),
    );
    info!(symbols = ?symbols, "Binance enabled");
    true
}

/// Register Binance Spot symbols into `cfg`, subscribing to both `@depth20@100ms`
/// and `@trade` streams. Returns `true` if any were added.
pub fn add_binance_spot(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::BinanceSpot).set_subscription_message(
            BinanceSubMsgBuilder::new()
                .with_orderbook_channel(&refs)
                .with_trade_channel(&refs)
                .build(),
        ),
    );
    info!(symbols = ?symbols, "Binance Spot enabled");
    true
}

/// Register OKX symbols into `cfg`. Returns `true` if any were added.
pub fn add_okx(cfg: &mut StreamSystemConfig, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return false;
    }
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Okx).set_subscription_message(
            OkxSubMsgBuilder::new()
                .with_orderbook_channel_multi(refs, "SPOT")
                .build(),
        ),
    );
    info!(symbols = ?symbols, "OKX enabled");
    true
}

/// Register Hyperliquid coins into `cfg`. Returns `true` if any were added.
pub fn add_hyperliquid(cfg: &mut StreamSystemConfig, coins: &[String]) -> bool {
    if coins.is_empty() {
        return false;
    }
    let refs: Vec<&str> = coins.iter().map(String::as_str).collect();
    cfg.with_exchange(
        ClientConfig::new(ExchangeName::Hyperliquid)
            .set_subscription_messages(HyperliquidSubMsgBuilder::new().with_coins(&refs).build()),
    );
    info!(coins = ?coins, "Hyperliquid enabled");
    true
}
