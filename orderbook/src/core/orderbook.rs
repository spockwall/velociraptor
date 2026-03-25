use super::types::*;
use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use std::collections::{BTreeMap, HashMap};
use tracing::{debug, info, warn};

// ── Orderbook ─────────────────────────────────────────────────────────────────

pub struct Orderbook {
    pub symbol: String,
    pub exchange: ExchangeName,
    pub last_update: DateTime<Utc>,
    /// Incremented on every `apply_update` call.
    pub sequence: u64,
    /// `sequence` of the last applied delta — used for gap detection.
    pub last_sequence: i64,

    pub bid_levels: BTreeMap<OrderedFloat<f64>, PriceLevel>,
    pub ask_levels: BTreeMap<OrderedFloat<f64>, PriceLevel>,

    pub is_initialized: bool,
}

impl Orderbook {
    pub fn new(symbol: String, exchange: ExchangeName) -> Self {
        Self {
            symbol,
            exchange,
            last_update: Utc::now(),
            sequence: 0,
            last_sequence: 0,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            is_initialized: false,
        }
    }

    // ── Core update dispatcher ────────────────────────────────────────────────

    pub fn apply_update(&mut self, update: OrderbookUpdate) {
        self.last_update = update.timestamp;
        self.sequence += 1;

        match update.action {
            DeltaAction::Partial => {
                if !self.is_initialized {
                    let n = update.payload.values().map(|v| v.len()).sum::<usize>();
                    info!(
                        "Snapshot for {} {} — {} levels",
                        self.exchange, self.symbol, n
                    );
                }
                self.clear();
                self.last_sequence = update.sequence;
                self.apply_payload(update.payload);
                self.is_initialized = true;
            }

            DeltaAction::Update => {
                if !self.is_initialized {
                    warn!("Update received before snapshot on {} {}, ignoring", self.exchange, self.symbol);
                    return;
                }
                if update.prev_sequence != 0 && update.prev_sequence != self.last_sequence {
                    warn!(
                        "Gap on {} {}: expected prev={} got {}",
                        self.exchange, self.symbol, self.last_sequence, update.prev_sequence
                    );
                }
                self.last_sequence = update.sequence;
                self.apply_payload(update.payload);
            }
        }
    }

    // ── Price-level upsert / delete ───────────────────────────────────────────

    fn apply_payload(&mut self, payload: HashMap<String, Vec<(f64, f64)>>) {
        if let Some(bids) = payload.get("bids") {
            for &(price, qty) in bids {
                self.apply_level(price, qty, OrderSide::Bid);
            }
        }
        if let Some(asks) = payload.get("asks") {
            for &(price, qty) in asks {
                self.apply_level(price, qty, OrderSide::Ask);
            }
        }
    }

    /// Upsert a price level; remove it when qty == 0.
    fn apply_level(&mut self, price: f64, qty: f64, side: OrderSide) {
        let key = OrderedFloat(price);
        let levels = match side {
            OrderSide::Bid => &mut self.bid_levels,
            OrderSide::Ask => &mut self.ask_levels,
        };

        if qty == 0.0 {
            if levels.remove(&key).is_some() {
                debug!("Removed {} level {} on {}", side, price, self.symbol);
            }
        } else {
            let level = levels.entry(key).or_insert(PriceLevel { price, total_qty: 0.0 });
            level.total_qty = qty;
            debug!("Upserted {} level {} qty {} on {}", side, price, qty, self.symbol);
        }
    }

    fn clear(&mut self) {
        self.bid_levels.clear();
        self.ask_levels.clear();
        self.last_sequence = 0;
        self.is_initialized = false;
    }

    // ── Market data queries ───────────────────────────────────────────────────

    pub fn best_bid(&self) -> Option<(f64, f64)> {
        self.bid_levels.iter().next_back().map(|(_, l)| (l.price, l.total_qty))
    }

    pub fn best_ask(&self) -> Option<(f64, f64)> {
        self.ask_levels.iter().next().map(|(_, l)| (l.price, l.total_qty))
    }

    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some(ask - bid),
            _ => None,
        }
    }

    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => Some((bid + ask) / 2.0),
            _ => None,
        }
    }

    pub fn depth(&self, levels: usize) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
        let bids = self.bid_levels.iter().rev().take(levels)
            .map(|(_, l)| (l.price, l.total_qty)).collect();
        let asks = self.ask_levels.iter().take(levels)
            .map(|(_, l)| (l.price, l.total_qty)).collect();
        (bids, asks)
    }

    pub fn avg_bid_price(&self, top_n: usize) -> f64 {
        if self.bid_levels.len() < top_n {
            warn!("Not enough bid levels");
            return 0.0;
        }
        let sum: f64 = self.bid_levels.iter().rev().take(top_n).map(|(_, l)| l.price).sum();
        sum / top_n as f64
    }

    pub fn avg_ask_price(&self, top_n: usize) -> f64 {
        if self.ask_levels.len() < top_n {
            warn!("Not enough ask levels");
            return 0.0;
        }
        let sum: f64 = self.ask_levels.iter().take(top_n).map(|(_, l)| l.price).sum();
        sum / top_n as f64
    }

    /// Volume-weighted average mid-price across the top `levels` levels.
    pub fn vamp(&self, levels: usize) -> f64 {
        let mut bid_vol = 0.0_f64;
        let mut ask_vol = 0.0_f64;
        let mut bid_fair = 0.0_f64;
        let mut ask_fair = 0.0_f64;

        for (_, l) in self.bid_levels.iter().rev().take(levels) {
            bid_vol += l.total_qty;
            bid_fair += l.price * l.total_qty;
        }
        for (_, l) in self.ask_levels.iter().take(levels) {
            ask_vol += l.total_qty;
            ask_fair += l.price * l.total_qty;
        }

        bid_fair /= bid_vol;
        ask_fair /= ask_vol;
        (bid_fair + ask_fair) / 2.0
    }

    /// Quantity-weighted mid-price using the best bid and ask.
    pub fn wmid(&self) -> f64 {
        if let (Some((bp, bq)), Some((ap, aq))) = (self.best_bid(), self.best_ask()) {
            let imbalance = bq / (bq + aq);
            (bp * (1.0 - imbalance) + ap * imbalance) / 2.0
        } else {
            0.0
        }
    }
}

// ── OrderBookDelta → OrderbookUpdate ─────────────────────────────────────────

impl From<OrderBookDelta> for OrderbookUpdate {
    fn from(delta: OrderBookDelta) -> Self {
        let timestamp =
            DateTime::from_timestamp_millis(delta.timestamp_ms).unwrap_or_else(Utc::now);
        OrderbookUpdate {
            action: delta.action,
            timestamp,
            sequence: delta.sequence,
            prev_sequence: delta.prev_sequence,
            payload: delta.payload,
        }
    }
}
