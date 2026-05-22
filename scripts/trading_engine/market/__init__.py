"""Market-data inputs.

  - `feed`: ZMQ SUB on the market PUB. Push model; keeps a
    `{(exchange, symbol) → latest Quote}` map populated for the strategy
    + observer to read.

Window discovery moved to `utils/windows.py`; the window dataclasses
(`PolymarketWindow` / `KalshiWindow`) live in `typings/window.py`.
"""

from .feed import MarketFeed, Quote, Trade

__all__ = [
    "MarketFeed",
    "Quote",
    "Trade",
]
