"""Market-data inputs.

  - `feed`:   ZMQ SUB transport. Enqueues `QuoteEvent` / `TradeEvent`
              / `RolloverEvent` onto the engine queue. Holds no cache.
  - `state`:  `MarketState` — the engine's live cross-source picture
              of the world. Owned by `Engine`, written by `Dispatcher`,
              read by `Strategy` callbacks.
  - `gamma`:  thin Polymarket REST helper to resolve a window's
              clobTokenIds from its `full_slug`. Strategies call this
              on rollover and stamp the result onto `MarketState`.

Window discovery lives in `utils/windows.py`; the window dataclasses
(`PolymarketWindow` / `KalshiWindow`) live in `typings/window.py`.
"""

from .feed import MarketFeed, Quote, Snapshot, Trade
from .gamma import current_full_slug, fetch_asset_ids, interval_secs_from_base_slug
from .state import MarketState

__all__ = [
    "MarketFeed",
    "MarketState",
    "Quote",
    "Snapshot",
    "Trade",
    "current_full_slug",
    "fetch_asset_ids",
    "interval_secs_from_base_slug",
]
