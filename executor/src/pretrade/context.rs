use libs::configs::PretradeLimits;
use libs::protocol::orders::PlaceOne;
use libs::protocol::ExchangeName;

/// Everything a rule needs to make a decision about one order.
/// Module-private outside `pretrade/`: only the runner constructs one.
pub(crate) struct PretradeContext<'a> {
    #[allow(dead_code)]
    pub(crate) exchange: ExchangeName,
    pub(crate) place: &'a PlaceOne,
    /// Best-effort count of currently-open orders for (exchange, symbol).
    pub(crate) open_orders: u32,
    /// Orders placed on (exchange, symbol) within the last rolling minute,
    /// *including* the one being evaluated.
    pub(crate) recent_order_count: u32,
    /// Reference price (e.g. mid from `bba:{exchange}:{symbol}`), if known.
    /// `None` → price-sanity rule passes (fail-open; never block solely
    /// because the reference is missing).
    pub(crate) reference_px: Option<f64>,
    pub(crate) limits: &'a PretradeLimits,
}

pub(crate) trait PretradeRule: Send + Sync {
    /// Stable identifier; used as the metrics label and in the rejection.
    fn name(&self) -> &'static str;
    /// `Ok(())` to pass, `Err(detail)` to reject with a human reason.
    fn check(&self, ctx: &PretradeContext) -> Result<(), String>;
}
