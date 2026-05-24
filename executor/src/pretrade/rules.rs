use super::context::{PretradeContext, PretradeRule};

/// Single-order quantity floor/ceiling.
pub(super) struct QtyBounds;
impl PretradeRule for QtyBounds {
    fn name(&self) -> &'static str {
        "qty_bounds"
    }
    fn check(&self, ctx: &PretradeContext) -> Result<(), String> {
        let q = ctx.place.qty;
        if let Some(min) = ctx.limits.min_qty {
            if q < min {
                return Err(format!("qty {q} below min {min}"));
            }
        }
        if let Some(max) = ctx.limits.max_qty {
            if q > max {
                return Err(format!("qty {q} above max {max}"));
            }
        }
        Ok(())
    }
}

/// Per-order notional (`px * qty`) ceiling.
pub(super) struct MaxNotional;
impl PretradeRule for MaxNotional {
    fn name(&self) -> &'static str {
        "max_notional"
    }
    fn check(&self, ctx: &PretradeContext) -> Result<(), String> {
        if let Some(max) = ctx.limits.max_notional {
            let notional = ctx.place.px * ctx.place.qty;
            if notional > max {
                return Err(format!(
                    "notional {notional:.4} (px {} * qty {}) above max {max}",
                    ctx.place.px, ctx.place.qty
                ));
            }
        }
        Ok(())
    }
}

/// Reject prices that deviate too far from a reference (fat-finger guard).
/// Fails open when no reference price is available.
pub(super) struct PriceSanity;
impl PretradeRule for PriceSanity {
    fn name(&self) -> &'static str {
        "price_sanity"
    }
    fn check(&self, ctx: &PretradeContext) -> Result<(), String> {
        let Some(max_pct) = ctx.limits.price_ref_max_deviation_pct else {
            return Ok(());
        };
        let Some(reference) = ctx.reference_px else {
            return Ok(());
        };
        if reference <= 0.0 {
            return Ok(());
        }
        let dev_pct = ((ctx.place.px - reference).abs() / reference) * 100.0;
        if dev_pct > max_pct {
            return Err(format!(
                "price {} deviates {dev_pct:.2}% from reference {reference} (max {max_pct}%)",
                ctx.place.px
            ));
        }
        Ok(())
    }
}

/// Rolling-minute placement rate limit per (exchange, symbol).
pub(super) struct RateLimit;
impl PretradeRule for RateLimit {
    fn name(&self) -> &'static str {
        "rate_limit"
    }
    fn check(&self, ctx: &PretradeContext) -> Result<(), String> {
        if let Some(max) = ctx.limits.max_orders_per_min {
            if ctx.recent_order_count > max {
                return Err(format!(
                    "{} orders in last 60s exceeds max {max}/min",
                    ctx.recent_order_count
                ));
            }
        }
        Ok(())
    }
}

/// Cap on concurrent open orders per (exchange, symbol). Best-effort count.
pub(super) struct MaxOpenOrders;
impl PretradeRule for MaxOpenOrders {
    fn name(&self) -> &'static str {
        "max_open_orders"
    }
    fn check(&self, ctx: &PretradeContext) -> Result<(), String> {
        if let Some(max) = ctx.limits.max_open_orders {
            if ctx.open_orders >= max {
                return Err(format!(
                    "{} open orders already at/above max {max}",
                    ctx.open_orders
                ));
            }
        }
        Ok(())
    }
}
