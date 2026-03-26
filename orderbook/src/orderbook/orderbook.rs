use crate::types::ExchangeName;
use crate::types::orderbook::*;
use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use std::collections::{BTreeMap, HashMap};
use tracing::{debug, info, warn};

#[derive(Clone, Debug)]
pub struct PriceLevel {
    pub price: f64,
    pub total_qty: f64,
    pub order_count: usize,
    pub orders: HashMap<String, f64>, // order_id -> qty
}

pub struct Orderbook {
    pub symbol: String,
    pub exchange: ExchangeName,
    pub last_update: DateTime<Utc>,
    pub sequence: u64,

    // Primary storage - by price level
    pub bid_levels: BTreeMap<OrderedFloat<f64>, PriceLevel>,
    pub ask_levels: BTreeMap<OrderedFloat<f64>, PriceLevel>,

    // State tracking
    pub is_initialized: bool,
}

impl Orderbook {
    pub fn new(symbol: String, exchange: ExchangeName) -> Self {
        Self {
            symbol,
            exchange: exchange.clone(),
            last_update: Utc::now(),
            sequence: 0,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            is_initialized: false,
        }
    }

    pub fn apply_update(&mut self, update: OrderbookUpdate) {
        self.last_update = update.timestamp;
        self.sequence += 1;

        match update.action {
            OrderbookAction::Snapshot => {
                // Every message from BitMart is a snapshot, so we need to check if the orderbook is initialized
                if !self.is_initialized {
                    info!(
                        "Applying snapshot for {}, Orderbook of {} isinitialized with {} entries",
                        self.exchange,
                        self.symbol,
                        update.orders.len()
                    );
                }
                self.clear();
                for order in update.orders {
                    self.add_order(order);
                }
                self.is_initialized = true;
            }
            OrderbookAction::Update => {
                if !self.is_initialized {
                    warn!("Received update before snapshot, ignoring");
                    return;
                }

                if self.exchange == ExchangeName::Okx {
                    for order in update.orders {
                        self.update_okx_order(order);
                    }
                } else {
                    for order in update.orders {
                        self.update_order(order);
                    }
                }
            }
            OrderbookAction::Insert => {
                if !self.is_initialized {
                    warn!("Received insert before snapshot, ignoring");
                    return;
                }
                for order in update.orders {
                    self.add_order(order);
                }
            }
            OrderbookAction::Delete => {
                if !self.is_initialized {
                    warn!("Received delete before snapshot, ignoring");
                    return;
                }
                for order in update.orders {
                    self.remove_order(order);
                }
            }
        }
    }

    fn update_okx_order(&mut self, order: GenericOrder) {
        let price_key = OrderedFloat(order.price);
        let side = self.parse_side(&order.side);

        let levels = match side {
            OrderSide::Bid => &mut self.bid_levels,
            OrderSide::Ask => &mut self.ask_levels,
        };

        if order.qty == 0.0 {
            // Delete the price level
            if levels.remove(&price_key).is_some() {
                debug!("Removed price level {} for {}", order.price, self.exchange);
            }
        } else {
            // Insert or update the price level
            let level = levels.entry(price_key).or_insert_with(|| PriceLevel {
                price: order.price,
                total_qty: 0.0,
                order_count: 0,
                orders: HashMap::new(),
            });

            // For OKX, we just replace the entire quantity at this price level
            level.total_qty = order.qty;
            level.order_count = 1; // OKX doesn't provide order count in the simplified view
            level.orders.clear();
            level.orders.insert(order.id.clone(), order.qty);

            debug!(
                "Updated OKX price level {} with qty {}",
                order.price, order.qty
            );
        }
    }

    fn add_order(&mut self, order: GenericOrder) {
        let price_key = OrderedFloat(order.price);
        let side = self.parse_side(&order.side);

        let levels = match side {
            OrderSide::Bid => &mut self.bid_levels,
            OrderSide::Ask => &mut self.ask_levels,
        };

        let level = levels.entry(price_key).or_insert_with(|| PriceLevel {
            price: order.price,
            total_qty: 0.0,
            order_count: 0,
            orders: HashMap::new(),
        });

        //// BitMEX always has order IDs
        //self.order_id_map.insert(
        //    order.id.clone(),
        //    OrderInfo {
        //        price: order.price,
        //        side: side.clone(),
        //        qty: order.qty,
        //    },
        //);

        level.orders.insert(order.id.clone(), order.qty);
        level.total_qty += order.qty;
        level.order_count += 1;

        debug!(
            "Added order {} at price {} qty {}",
            order.id, order.price, order.qty
        );
    }

    fn update_order(&mut self, order: GenericOrder) {
        //// BitMEX update means size changed for existing order ID
        //if let Some(existing_info) = self.order_id_map.get_mut(&order.id) {
        //    let old_qty = existing_info.qty;
        //    let price_key = OrderedFloat(existing_info.price);

        //    let levels = match existing_info.side {
        //        OrderSide::Bid => &mut self.bid_levels,
        //        OrderSide::Ask => &mut self.ask_levels,
        //    };

        //    if let Some(level) = levels.get_mut(&price_key) {
        //        // Update quantity
        //        level.total_qty = level.total_qty - old_qty + order.qty;
        //        level.orders.insert(order.id.clone(), order.qty);
        //        existing_info.qty = order.qty;

        //        debug!(
        //            "Updated order {} from qty {} to {}",
        //            order.id, old_qty, order.qty
        //        );
        //    }
        //} else {
        //    warn!("Update for unknown order ID: {}", order.id);
        //}
    }

    fn remove_order(&mut self, order: GenericOrder) {
        //if let Some(info) = self.order_id_map.remove(&order.id) {
        //    let price_key = OrderedFloat(info.price);
        //    let levels = match info.side {
        //        OrderSide::Bid => &mut self.bid_levels,
        //        OrderSide::Ask => &mut self.ask_levels,
        //    };

        //    if let Some(level) = levels.get_mut(&price_key) {
        //        level.total_qty -= info.qty;
        //        level.order_count -= 1;
        //        level.orders.remove(&order.id);

        //        if level.order_count == 0 {
        //            levels.remove(&price_key);
        //        }

        //        debug!("Removed order {} at price {}", order.id, info.price);
        //    }
        //} else {
        //    warn!("Delete for unknown order ID: {}", order.id);
        //}
    }

    fn clear(&mut self) {
        self.bid_levels.clear();
        self.ask_levels.clear();
        self.is_initialized = false;
    }

    fn parse_side(&self, side: &str) -> OrderSide {
        match side {
            "Bid" | "Buy" => OrderSide::Bid,
            "Ask" | "Sell" => OrderSide::Ask,
            _ => {
                warn!("Unknown side: {}, defaulting to Ask", side);
                OrderSide::Ask
            }
        }
    }

    pub fn best_bid(&self) -> Option<(f64, f64)> {
        self.bid_levels
            .iter()
            .next_back()
            .map(|(_, level)| (level.price, level.total_qty))
    }

    pub fn best_ask(&self) -> Option<(f64, f64)> {
        self.ask_levels
            .iter()
            .next()
            .map(|(_, level)| (level.price, level.total_qty))
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
        let bids: Vec<_> = self
            .bid_levels
            .iter()
            .rev()
            .take(levels)
            .map(|(_, level)| (level.price, level.total_qty))
            .collect();

        let asks: Vec<_> = self
            .ask_levels
            .iter()
            .take(levels)
            .map(|(_, level)| (level.price, level.total_qty))
            .collect();

        (bids, asks)
    }

    /// Calculates the average bid price for the top `number` levels.
    pub fn avg_bid_price(&self, top_n: usize) -> f64 {
        if self.bid_levels.len() < top_n {
            warn!("Not enough bid levels to calculate average bid price");
            return 0.0;
        }

        let price_sum = self
            .bid_levels
            .iter()
            .rev()
            .take(top_n)
            .map(|(_, level)| level.price)
            .sum::<f64>();
        price_sum / top_n as f64
    }

    /// Calculates the average ask price for the top `number` levels.
    pub fn avg_ask_price(&self, top_n: usize) -> f64 {
        if self.ask_levels.len() < top_n {
            warn!("Not enough ask levels to calculate average ask price");
            return 0.0;
        }

        let price_sum = self
            .ask_levels
            .iter()
            .take(top_n)
            .map(|(_, level)| level.price)
            .sum::<f64>();
        price_sum / top_n as f64
    }

    /// Steps:
    /// 1. Sum the quantities for the top `depth` bids and asks separately.
    /// 2. Calculate the weighted price for each bid and ask up to the specified depth, using their quantities as weights.
    /// 3. Compute the fair value for bids and asks by multiplying each price by its relative weight and summing the results.
    /// 4. Calculate the VAMP as the average of the bid and ask fair values.
    pub fn vamp(&self, levels: usize) -> f64 {
        let mut bid_volume = 0.0;
        let mut ask_volume = 0.0;
        let mut bid_fair = 0.0;
        let mut ask_fair = 0.0;

        for (_, level) in self.bid_levels.iter().rev().take(levels) {
            bid_volume += level.total_qty;
            bid_fair += level.price * level.total_qty;
        }

        for (_, level) in self.ask_levels.iter().take(levels) {
            ask_volume += level.total_qty;
            ask_fair += level.price * level.total_qty;
        }

        bid_fair /= bid_volume;
        ask_fair /= ask_volume;
        return (bid_fair + ask_fair) / 2.0;
    }

    /// Calculates the weighted mid-price considering the quantities of the best bid and ask.
    /// Steps:
    /// 1. Calculate the bid-ask imbalance by dividing the best bid quantity by the sum of best bid and ask quantities.
    /// 2. Compute the weighted mid-price by applying the imbalance to the best bid and ask prices.
    /// 3. If no bid or ask, return 0.0 temporarily.
    pub fn wmid(&self) -> f64 {
        let (best_bid, best_ask) = (self.best_bid(), self.best_ask());
        if let (Some((bid_price, bid_qty)), Some((ask_price, ask_qty))) = (best_bid, best_ask) {
            let imbalance = bid_qty / (bid_qty + ask_qty);
            let weighted_bid = bid_price * (1.0 - imbalance);
            let weighted_ask = ask_price * imbalance;
            return (weighted_bid + weighted_ask) / 2.0;
        }
        return 0.0;
    }
}
