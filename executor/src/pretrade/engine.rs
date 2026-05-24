use super::context::{PretradeContext, PretradeRule};
use super::rules::{MaxNotional, MaxOpenOrders, PriceSanity, QtyBounds, RateLimit};

pub struct PretradeEngine {
    rules: Vec<Box<dyn PretradeRule>>,
}

impl Default for PretradeEngine {
    fn default() -> Self {
        Self {
            rules: vec![
                Box::new(QtyBounds),
                Box::new(MaxNotional),
                Box::new(PriceSanity),
                Box::new(RateLimit),
                Box::new(MaxOpenOrders),
            ],
        }
    }
}

impl PretradeEngine {
    /// Run every rule; return the first failure as `(rule_name, detail)`.
    pub fn evaluate(&self, ctx: &PretradeContext) -> Result<(), (&'static str, String)> {
        for rule in &self.rules {
            if let Err(detail) = rule.check(ctx) {
                return Err((rule.name(), detail));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::configs::PretradeLimits;
    use libs::protocol::orders::{OrderKind, PlaceOne, Side, Tif};
    use libs::protocol::ExchangeName;

    fn place(px: f64, qty: f64) -> PlaceOne {
        PlaceOne {
            client_oid: "t".into(),
            symbol: "TOK".into(),
            side: Side::Buy,
            kind: OrderKind::Limit,
            px,
            qty,
            tif: Tif::Gtc,
        }
    }

    fn ctx<'a>(
        p: &'a PlaceOne,
        l: &'a PretradeLimits,
        reference_px: Option<f64>,
    ) -> PretradeContext<'a> {
        PretradeContext {
            exchange: ExchangeName::Polymarket,
            place: p,
            open_orders: 0,
            recent_order_count: 1,
            reference_px,
            limits: l,
        }
    }

    #[test]
    fn passes_when_all_limits_none() {
        let p = place(0.5, 10.0);
        let l = PretradeLimits::default();
        assert!(PretradeEngine::default().evaluate(&ctx(&p, &l, None)).is_ok());
    }

    #[test]
    fn max_qty_rejects_over_and_passes_at_boundary() {
        let l = PretradeLimits {
            max_qty: Some(10.0),
            ..Default::default()
        };
        let eng = PretradeEngine::default();
        let over = place(0.5, 10.01);
        let (rule, _) = eng.evaluate(&ctx(&over, &l, None)).unwrap_err();
        assert_eq!(rule, "qty_bounds");
        let at = place(0.5, 10.0);
        assert!(eng.evaluate(&ctx(&at, &l, None)).is_ok());
    }

    #[test]
    fn min_qty_rejects_below() {
        let l = PretradeLimits {
            min_qty: Some(5.0),
            ..Default::default()
        };
        let p = place(0.5, 4.99);
        assert_eq!(
            PretradeEngine::default()
                .evaluate(&ctx(&p, &l, None))
                .unwrap_err()
                .0,
            "qty_bounds"
        );
    }

    #[test]
    fn max_notional_rejects() {
        let l = PretradeLimits {
            max_notional: Some(5.0),
            ..Default::default()
        };
        let p = place(0.6, 10.0);
        assert_eq!(
            PretradeEngine::default()
                .evaluate(&ctx(&p, &l, None))
                .unwrap_err()
                .0,
            "max_notional"
        );
    }

    #[test]
    fn price_sanity_fails_open_without_reference() {
        let l = PretradeLimits {
            price_ref_max_deviation_pct: Some(5.0),
            ..Default::default()
        };
        let p = place(0.99, 1.0);
        assert!(PretradeEngine::default().evaluate(&ctx(&p, &l, None)).is_ok());
    }

    #[test]
    fn price_sanity_rejects_far_from_reference() {
        let l = PretradeLimits {
            price_ref_max_deviation_pct: Some(5.0),
            ..Default::default()
        };
        let p = place(0.60, 1.0);
        let (rule, _) = PretradeEngine::default()
            .evaluate(&ctx(&p, &l, Some(0.50)))
            .unwrap_err();
        assert_eq!(rule, "price_sanity");
    }

    #[test]
    fn rate_limit_rejects_when_over() {
        let l = PretradeLimits {
            max_orders_per_min: Some(3),
            ..Default::default()
        };
        let p = place(0.5, 1.0);
        let mut c = ctx(&p, &l, None);
        c.recent_order_count = 4;
        assert_eq!(
            PretradeEngine::default().evaluate(&c).unwrap_err().0,
            "rate_limit"
        );
    }

    #[test]
    fn max_open_orders_rejects_at_cap() {
        let l = PretradeLimits {
            max_open_orders: Some(2),
            ..Default::default()
        };
        let p = place(0.5, 1.0);
        let mut c = ctx(&p, &l, None);
        c.open_orders = 2;
        assert_eq!(
            PretradeEngine::default().evaluate(&c).unwrap_err().0,
            "max_open_orders"
        );
    }
}
