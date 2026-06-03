"""Pure data types for the trading engine.

  - `events`: immutable event dataclasses carried on the engine queue.
  - `orders`: typed views over the executor `OrderAck` / `FillInfo` replies.
  - `state`:  per-strategy state container (trade history + order ledger).
  - `window`: rolling-window dataclasses (PolymarketWindow / KalshiWindow).

No behavior or threading here — the dispatcher that consumes events lives
in `trading/dispatcher.py`; window discovery lives in `utils/windows.py`.
"""

from __future__ import annotations

from .events import (
    Event,
    FillEvent,
    OrderUpdateEvent,
    PolyMakerOrder,
    QuoteEvent,
    RolloverEvent,
    ShutdownEvent,
    TradeEvent,
)
from .orders import FillInfo, OrderAck
from .state import OrderLedger, OrderRecord, StrategyState
from .window import KalshiWindow, PolymarketWindow

__all__ = [
    "Event",
    "FillEvent",
    "FillInfo",
    "OrderAck",
    "OrderLedger",
    "OrderRecord",
    "OrderUpdateEvent",
    "PolyMakerOrder",
    "QuoteEvent",
    "RolloverEvent",
    "ShutdownEvent",
    "StrategyState",
    "TradeEvent",

    # Window
    "KalshiWindow",
    "PolymarketWindow",
]
