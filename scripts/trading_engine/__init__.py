"""Velociraptor Python trading engine.

One engine = one strategy. Launch one process per market.

Available strategies (select with `--strategy`):

  observe   — passive multi-exchange watcher; no orders.
  probe     — place + cancel at the price floor on every quote.
  fill_once — cross the spread, fill once per window.
  one_shot  — place once, cancel once, terminate.
  momentum  — Binance signal → one small Polymarket position.

Run with:

    python -m scripts.trading_engine --strategy observe
    python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m

See `README.md` for the full architecture and CLI reference.
"""

from .app import Engine, main
from .io import (
    ExecutorClient,
    EventHandler,
    OrderRouter,
    UserFeed,
)
from .market import MarketFeed, MarketState, Quote, Trade
from .trading import (
    Dispatcher,
    ObserveStrategy,
    Strategy,
    WindowStrategy,
    available_strategies,
    build_strategy,
)
from .typings.state import OrderLedger, OrderRecord, StrategyState
from .typings.window import KalshiWindow, PolymarketWindow as MarketWindow
from .utils.windows import (
    discover_kalshi,
    discover_polymarket as discover,
)

__all__ = [
    "Dispatcher",
    "Engine",
    "EventHandler",
    "ExecutorClient",
    "KalshiWindow",
    "MarketFeed",
    "MarketState",
    "MarketWindow",
    "ObserveStrategy",
    "OrderLedger",
    "OrderRecord",
    "OrderRouter",
    "Quote",
    "Strategy",
    "StrategyState",
    "Trade",
    "UserFeed",
    "WindowStrategy",
    "available_strategies",
    "build_strategy",
    "discover",
    "discover_kalshi",
    "main",
]
