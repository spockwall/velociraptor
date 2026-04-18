pub mod endpoints;
pub mod errors;
pub mod orderbook;
pub mod stream;

pub use libs::protocol::OrderbookSnapshot;
pub use stream::{PriceLevelTuple, StreamEvent, StreamEventSource};

use crate::orderbook::Orderbook;

/// Build a `OrderbookSnapshot` from an `Orderbook` at the given depth.
pub fn snapshot_from(book: &Orderbook, depth: usize) -> OrderbookSnapshot {
    let (bids, asks) = book.depth(depth);
    OrderbookSnapshot {
        exchange: book.exchange.clone(),
        symbol: book.symbol.clone(),
        sequence: book.sequence,
        timestamp: book.last_update,
        best_bid: book.best_bid(),
        best_ask: book.best_ask(),
        spread: book.spread(),
        mid: book.mid_price(),
        wmid: book.wmid(),
        bids,
        asks,
    }
}
