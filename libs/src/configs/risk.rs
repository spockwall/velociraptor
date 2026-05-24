//! Per-(exchange, symbol) risk config for the executor.
//!
//! `PretradeLimits` holds the pre-trade rule fields. Future risk categories
//! (position, exposure, …) will live alongside it on `ExchangeRisk`.
//!
//! Shape (YAML):
//!
//! ```yaml
//! risk:
//!   enabled: true
//!   exchanges:
//!     polymarket:
//!       default: { max_qty: 100, max_notional: 500 }
//!       symbols:
//!         "12345...":            # token id / slug / ticker
//!           max_qty: 50
//!     kalshi:
//!       default: { max_qty: 200 }
//! ```
//!
//! Resolution: a symbol override is **per-field merged** over the exchange
//! `default` (a symbol that sets only `max_qty` still inherits the default's
//! `max_notional`). If the exchange has no entry at all, `resolve` returns
//! `None` and the gate lets the order through (unrestricted, by design).

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RiskConfig {
    /// Master switch. When false the executor skips the risk gate entirely.
    pub enabled: bool,
    /// Keyed by `ExchangeName::to_str()` (lowercase: "polymarket", "kalshi", …).
    pub exchanges: HashMap<String, ExchangeRisk>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ExchangeRisk {
    /// Applied to any symbol on this exchange without an explicit override.
    pub default: PretradeLimits,
    /// Per-symbol overrides, keyed by the order's `symbol` (token id / slug /
    /// ticker, exactly as it arrives in `PlaceOne.symbol`).
    pub symbols: HashMap<String, PretradeLimits>,
}

/// Every field is optional — `None` means "this rule is disabled for the
/// resolved scope". Add new optional fields here as new rules are introduced;
/// the risk engine skips any rule whose backing field is `None`.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PretradeLimits {
    /// Reject a single order with `qty` below this floor.
    pub min_qty: Option<f64>,
    /// Reject a single order with `qty` above this ceiling.
    pub max_qty: Option<f64>,
    /// Reject when `px * qty` exceeds this (per single order).
    pub max_notional: Option<f64>,
    /// Reject when `|px - reference_px| / reference_px * 100` exceeds this.
    pub price_ref_max_deviation_pct: Option<f64>,
    /// Reject when more than this many orders were placed on the
    /// (exchange, symbol) within the last rolling minute.
    pub max_orders_per_min: Option<u32>,
    /// Reject when the (exchange, symbol) already has at least this many
    /// open orders (best-effort count — see executor docs).
    pub max_open_orders: Option<u32>,
}

impl PretradeLimits {
    /// Overlay `over` on top of `self`, field by field: any field set in
    /// `over` wins; unset fields fall back to `self`.
    fn merged_with(&self, over: &PretradeLimits) -> PretradeLimits {
        PretradeLimits {
            min_qty: over.min_qty.or(self.min_qty),
            max_qty: over.max_qty.or(self.max_qty),
            max_notional: over.max_notional.or(self.max_notional),
            price_ref_max_deviation_pct: over
                .price_ref_max_deviation_pct
                .or(self.price_ref_max_deviation_pct),
            max_orders_per_min: over.max_orders_per_min.or(self.max_orders_per_min),
            max_open_orders: over.max_open_orders.or(self.max_open_orders),
        }
    }
}

impl RiskConfig {
    /// Effective limits for `(exchange, symbol)`:
    ///   - exchange has a symbol override → exchange.default ⊕ symbol override
    ///   - exchange present, no symbol override → exchange.default
    ///   - exchange absent → `None` (gate passes the order through)
    ///
    /// `exchange` must be the lowercase `ExchangeName::to_str()` form.
    pub fn resolve(&self, exchange: &str, symbol: &str) -> Option<PretradeLimits> {
        let ex = self.exchanges.get(exchange)?;
        match ex.symbols.get(symbol) {
            Some(sym) => Some(ex.default.merged_with(sym)),
            None => Some(ex.default.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RiskConfig {
        let mut symbols = HashMap::new();
        symbols.insert(
            "TOK".to_string(),
            PretradeLimits {
                max_qty: Some(50.0),
                ..Default::default()
            },
        );
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "polymarket".to_string(),
            ExchangeRisk {
                default: PretradeLimits {
                    max_qty: Some(100.0),
                    max_notional: Some(500.0),
                    ..Default::default()
                },
                symbols,
            },
        );
        RiskConfig {
            enabled: true,
            exchanges,
        }
    }

    #[test]
    fn unknown_exchange_returns_none() {
        assert!(cfg().resolve("binance", "anything").is_none());
    }

    #[test]
    fn no_symbol_override_uses_default() {
        let l = cfg().resolve("polymarket", "OTHER").unwrap();
        assert_eq!(l.max_qty, Some(100.0));
        assert_eq!(l.max_notional, Some(500.0));
    }

    #[test]
    fn symbol_override_is_per_field_merged() {
        let l = cfg().resolve("polymarket", "TOK").unwrap();
        // Overridden field wins …
        assert_eq!(l.max_qty, Some(50.0));
        // … but unset fields still inherit the exchange default.
        assert_eq!(l.max_notional, Some(500.0));
    }
}
