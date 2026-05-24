//! Pre-trade risk runner. Walks the configured rules for every
//! `Place` / `PlaceBatch` leg and returns the first rejection as
//! `(rule_name, detail)`. Cancels / update / heartbeat skip the runner.

use libs::protocol::events::BbaPayload;
use libs::protocol::orders::{OrderAction, OrderRequest, PlaceOne};
use libs::redis_client::keys::RedisKey;
use libs::redis_client::RedisHandle;

use super::context::{PretradeContext, PretradeRule};
use super::rules::{MaxNotional, MaxOpenOrders, PriceSanity, QtyBounds, RateLimit};
use super::state::PretradeState;

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
    /// Evaluate every rule for every Place leg of `req`. Returns
    /// `Some((rule, detail))` on the first rejection, `None` if the order
    /// passes (or risk is disabled, or the (exchange, symbol) is
    /// unconfigured). Cancels / update / heartbeat always return `None`.
    pub async fn run(
        &self,
        req: &OrderRequest,
        state: &PretradeState,
        redis: Option<&RedisHandle>,
    ) -> Option<(&'static str, String)> {
        let places: Vec<&PlaceOne> = match &req.action {
            OrderAction::Place(p) => vec![p],
            OrderAction::PlaceBatch { orders } => orders.iter().collect(),
            _ => return None,
        };

        let cfg = state.snapshot();
        if !cfg.enabled {
            return None;
        }
        let ex_str = req.exchange.to_str();

        for p in places {
            let Some(limits) = cfg.resolve(ex_str, &p.symbol) else {
                continue;
            };
            let reference_px = reference_price(redis, ex_str, &p.symbol).await;
            let open_orders = state.open_count(req.exchange, &p.symbol);
            // Counts this prospective placement in the rolling window so a
            // burst is caught on the order that crosses the threshold.
            let recent_order_count = state.record_and_count_rate(req.exchange, &p.symbol);
            let ctx = PretradeContext {
                exchange: req.exchange,
                place: p,
                open_orders,
                recent_order_count,
                reference_px,
                limits: &limits,
            };
            if let Some(hit) = self.check(&ctx) {
                return Some(hit);
            }
        }
        None
    }

    fn check(&self, ctx: &PretradeContext) -> Option<(&'static str, String)> {
        for rule in &self.rules {
            if let Err(detail) = rule.check(ctx) {
                return Some((rule.name(), detail));
            }
        }
        None
    }
}

/// Mid price from `bba:{exchange}:{symbol}` if present, else `None` (the
/// price-sanity rule then fails open). Any Redis/decode error → `None`.
async fn reference_price(redis: Option<&RedisHandle>, exchange: &str, symbol: &str) -> Option<f64> {
    let bytes = redis?.get_raw(&RedisKey::bba(exchange, symbol)).await?;
    let bba: BbaPayload = rmp_serde::from_slice(&bytes).ok()?;
    match (bba.best_bid, bba.best_ask) {
        (Some((bid, _)), Some((ask, _))) => Some((bid + ask) / 2.0),
        (Some((bid, _)), None) => Some(bid),
        (None, Some((ask, _))) => Some(ask),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libs::configs::{ExchangeRisk, PretradeLimits, RiskConfig};
    use libs::protocol::orders::{OrderKind, Side, Tif};
    use libs::protocol::ExchangeName;
    use std::collections::HashMap;

    fn place_req(qty: f64, px: f64) -> OrderRequest {
        OrderRequest {
            req_id: 1,
            exchange: ExchangeName::Polymarket,
            action: OrderAction::Place(PlaceOne {
                client_oid: "t".into(),
                symbol: "TOK".into(),
                side: Side::Buy,
                kind: OrderKind::Limit,
                px,
                qty,
                tif: Tif::Gtc,
            }),
        }
    }

    fn cfg(limits: PretradeLimits) -> RiskConfig {
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "polymarket".to_string(),
            ExchangeRisk {
                default: limits,
                symbols: HashMap::new(),
            },
        );
        RiskConfig {
            enabled: true,
            exchanges,
        }
    }

    #[tokio::test]
    async fn passes_when_disabled() {
        let st = PretradeState::default();
        // disabled by default — even oversize qty passes
        assert!(PretradeEngine::default()
            .run(&place_req(1e9, 0.5), &st, None)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn passes_when_unconfigured_exchange() {
        let st = PretradeState::default();
        st.swap(RiskConfig {
            enabled: true,
            exchanges: HashMap::new(),
        });
        assert!(PretradeEngine::default()
            .run(&place_req(1e9, 0.5), &st, None)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn max_qty_rejects_over_passes_at_boundary() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            max_qty: Some(10.0),
            ..Default::default()
        }));
        let r = PretradeEngine::default();
        let (rule, _) = r.run(&place_req(10.01, 0.5), &st, None).await.unwrap();
        assert_eq!(rule, "qty_bounds");
        assert!(r.run(&place_req(10.0, 0.5), &st, None).await.is_none());
    }

    #[tokio::test]
    async fn min_qty_rejects_below() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            min_qty: Some(5.0),
            ..Default::default()
        }));
        let (rule, _) = PretradeEngine::default()
            .run(&place_req(4.99, 0.5), &st, None)
            .await
            .unwrap();
        assert_eq!(rule, "qty_bounds");
    }

    #[tokio::test]
    async fn max_notional_rejects() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            max_notional: Some(5.0),
            ..Default::default()
        }));
        let (rule, _) = PretradeEngine::default()
            .run(&place_req(10.0, 0.6), &st, None)
            .await
            .unwrap();
        assert_eq!(rule, "max_notional");
    }

    #[tokio::test]
    async fn price_sanity_fails_open_without_reference() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            price_ref_max_deviation_pct: Some(5.0),
            ..Default::default()
        }));
        assert!(PretradeEngine::default()
            .run(&place_req(1.0, 0.99), &st, None)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn rate_limit_rejects_when_over() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            max_orders_per_min: Some(2),
            ..Default::default()
        }));
        let r = PretradeEngine::default();
        // First three placements bump the rolling count; the third crosses 2.
        assert!(r.run(&place_req(1.0, 0.5), &st, None).await.is_none());
        assert!(r.run(&place_req(1.0, 0.5), &st, None).await.is_none());
        let (rule, _) = r.run(&place_req(1.0, 0.5), &st, None).await.unwrap();
        assert_eq!(rule, "rate_limit");
    }

    #[tokio::test]
    async fn max_open_orders_rejects_at_cap() {
        let st = PretradeState::default();
        st.swap(cfg(PretradeLimits {
            max_open_orders: Some(2),
            ..Default::default()
        }));
        st.incr_open(ExchangeName::Polymarket, "TOK");
        st.incr_open(ExchangeName::Polymarket, "TOK");
        let (rule, _) = PretradeEngine::default()
            .run(&place_req(1.0, 0.5), &st, None)
            .await
            .unwrap();
        assert_eq!(rule, "max_open_orders");
    }
}
