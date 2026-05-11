"""Market-data inputs.

Two modules:

  - `discovery`: HTTP poll of the backend for active Polymarket / Kalshi
    windows. Pull model, called every `--rediscover-secs` from the engine.
  - `feed`: ZMQ SUB on the market PUB. Push model; keeps a
    `{(exchange, symbol) → latest Quote}` map populated for the strategy
    + observer to read.
"""

from .discovery import (
    KalshiWindow,
    MarketWindow,
    discover,
    discover_kalshi,
    fetch_kalshi_markets,
    fetch_markets,
    group_into_windows,
)
from .feed import MarketFeed, Quote

__all__ = [
    "KalshiWindow",
    "MarketFeed",
    "MarketWindow",
    "Quote",
    "discover",
    "discover_kalshi",
    "fetch_kalshi_markets",
    "fetch_markets",
    "group_into_windows",
]
