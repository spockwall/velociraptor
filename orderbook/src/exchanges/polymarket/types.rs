use serde::Deserialize;

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
    /// "0" means the level has been removed from the book.
    pub size: String,
    /// "BUY" = bid side, "SELL" = ask side.
    pub side: String,
}

/// Incremental diff — emitted on order placement or cancellation.
/// One message may carry changes for multiple assets.
#[derive(Debug, Deserialize)]
pub struct PolymarketPriceChangeEvent {
    pub timestamp: String,
    pub price_changes: Vec<PolymarketPriceChange>,
}
