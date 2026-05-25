"""Market-data inputs.

  - `feed`:  ZMQ SUB transport. Enqueues `QuoteEvent` / `TradeEvent` /
             `RolloverEvent` onto the engine queue. Holds no cache.
  - `state`: `MarketState` — the engine's live cross-source picture of
             the world. Owned by `Engine`, written by `Dispatcher`,
             read by `Strategy` callbacks.

Window discovery lives in `utils/windows.py`; the window dataclasses
(`PolymarketWindow` / `KalshiWindow`) live in `typings/window.py`.
"""

from .feed import MarketFeed, Quote, Snapshot, Trade
from .state import MarketState

__all__ = [
    "MarketFeed",
    "MarketState",
    "Quote",
    "Snapshot",
    "Trade",
]
