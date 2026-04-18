pub mod endpoints;
pub mod errors;
pub mod events;
pub mod orderbook;

pub use events::{PriceLevelTuple, StreamEvent, StreamEventSource};
pub use libs::protocol::StreamSnapshot;

use crate::orderbook::Orderbook;

/// Build a `StreamSnapshot` from an `Orderbook` at the given depth.
pub fn snapshot_from_book(book: &Orderbook, depth: usize) -> StreamSnapshot {
    let (bids, asks) = book.depth(depth);
    StreamSnapshot {
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
