use crate::orderbook::Orderbook;
use tracing::debug;

pub(crate) fn log_orderbook_bba(key: &str, book: &Orderbook) {
    if book.sequence % 100 != 0 {
        return;
    }
    if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) {
        debug!(
            "{} - Bid: {:.2} x {:.2}, Ask: {:.2} x {:.2}, Spread: {:.2}",
            key,
            bid.0,
            bid.1,
            ask.0,
            ask.1,
            ask.0 - bid.0
        );
    }
}
