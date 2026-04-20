use serde::Deserialize;

// ═══════════════════════════════════════════════════════════════════════════════
// Market channel wire types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolymarketEventType {
    Book,
    PriceChange,
}

#[derive(Debug, Deserialize)]
pub struct PolymarketLevel {
    pub price: String,
    pub size: String,
}

/// Full orderbook snapshot — emitted on subscribe and after each trade.
#[derive(Debug, Deserialize)]
pub struct PolymarketBookEvent {
    pub asset_id: String,
    pub bids: Vec<PolymarketLevel>,
    pub asks: Vec<PolymarketLevel>,
    pub timestamp: String,
}

/// Single price level change within a `price_change` event.
#[derive(Debug, Deserialize)]
pub struct PolymarketPriceChange {
    pub asset_id: String,
    pub price: String,
    /// `"0"` means the level has been removed from the book.
    pub size: String,
    /// `"BUY"` = bid side, `"SELL"` = ask side.
    pub side: String,
}

/// Incremental diff — emitted on order placement or cancellation.
/// One message may carry changes for multiple assets.
#[derive(Debug, Deserialize)]
pub struct PolymarketPriceChangeEvent {
    pub timestamp: String,
    pub price_changes: Vec<PolymarketPriceChange>,
}

/// Public market `last_trade_price` event — emitted when a maker and taker
/// order is matched.
#[derive(Debug, Deserialize)]
pub struct PolyLastTradePriceEvent {
    pub asset_id: String,
    pub price: String,
    pub size: String,
    /// Taker side: `"BUY"` or `"SELL"`.
    pub side: String,
    #[serde(default)]
    pub fee_rate_bps: String,
    #[serde(default)]
    pub market: String,
    /// Unix milliseconds as a string.
    pub timestamp: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// User channel wire types
// ═══════════════════════════════════════════════════════════════════════════════

/// Raw order-update event from the Polymarket user channel.
///
/// `type` field values: `"PLACEMENT"`, `"CANCELLATION"`, `"UPDATE"`, `"MATCH"`.
/// Maps to lifecycle status for our `OrderStatus`.
#[derive(Debug, Deserialize)]
pub struct PolyOrderEvent {
    /// Exchange-assigned order ID.
    pub id: String,
    /// Asset (token) ID this order is on.
    pub asset_id: String,
    /// Order side: `"BUY"` or `"SELL"`.
    pub side: String,
    /// Limit price as a string decimal.
    pub price: String,
    /// Original order size.
    pub original_size: String,
    /// Amount already matched/filled.
    pub size_matched: String,
    /// Order lifecycle type: `"PLACEMENT"`, `"CANCELLATION"`, `"UPDATE"`, `"MATCH"`.
    #[serde(rename = "type", default)]
    pub order_type: String,
    /// Owner/trader identifier (Polymarket user UUID).
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub _timestamp: String,
}

/// Raw trade/fill event from the Polymarket user channel.
///
/// The taker order is identified by `taker_order_id`; maker orders are
/// listed in `maker_orders`.
#[derive(Debug, Deserialize)]
pub struct PolyTradeEvent {
    /// Trade ID (UUID).
    pub id: String,
    /// Asset (token) ID.
    pub asset_id: String,
    /// Trade side from the taker's perspective: `"BUY"` or `"SELL"`.
    pub side: String,
    /// Execution price.
    pub price: String,
    /// Total filled size.
    pub size: String,
    /// Taker order ID.
    #[serde(default)]
    pub taker_order_id: String,
    /// Nested maker order details.
    #[serde(default)]
    pub maker_orders: Vec<PolyMakerOrder>,
    /// Trade status, e.g. `"MATCHED"`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub _timestamp: String,
}

/// One maker order entry nested inside a `PolyTradeEvent`.
#[derive(Debug, Deserialize)]
pub struct PolyMakerOrder {
    pub order_id: String,
    pub asset_id: String,
    pub matched_amount: String,
    pub price: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub owner: String,
}
