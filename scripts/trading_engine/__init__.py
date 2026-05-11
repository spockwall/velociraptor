"""Velociraptor Python trading engine.

Three steps (selected by `--step`):

  0 — passive multi-exchange orderbook observer; no orders.
  1 — far-from-touch place + cancel probe; no fills expected.
  2 — tight quotes; fill once per side per window ($10 notional).

Run with:

    python -m scripts.trading_engine --step 1

See `README.md` for the full architecture and CLI reference.
"""

from .app import Engine, main
from .io import (
    ExecutorClient,
    EventHandler,
    OrderRouter,
    UserFeed,
)
from .market import (
    KalshiWindow,
    MarketFeed,
    MarketWindow,
    Quote,
    discover,
    discover_kalshi,
)
from .trading import (
    Observer,
    SideState,
    TrackedSymbol,
    WindowStrategy,
)

__all__ = [
    "Engine",
    "ExecutorClient",
    "EventHandler",
    "KalshiWindow",
    "MarketFeed",
    "MarketWindow",
    "Observer",
    "OrderRouter",
    "Quote",
    "SideState",
    "TrackedSymbol",
    "UserFeed",
    "WindowStrategy",
    "discover",
    "discover_kalshi",
    "main",
]
