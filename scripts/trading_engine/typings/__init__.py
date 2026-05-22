"""Pure data types for the trading engine.

  - `events`: immutable event dataclasses carried on the engine queue.
  - `state`:  per-strategy state container (trade history + order ledger).

No behavior or threading here — the dispatcher that consumes events lives
in `dispatcher.py`.
"""

from __future__ import annotations

from .events import (
    Event,
    FillEvent,
    OrderUpdateEvent,
    QuoteEvent,
    ShutdownEvent,
    TimerEvent,
    TradeEvent,
)
from .state import OrderLedger, OrderRecord, StrategyState

__all__ = [
    "Event",
    "FillEvent",
    "OrderLedger",
    "OrderRecord",
    "OrderUpdateEvent",
    "QuoteEvent",
    "ShutdownEvent",
    "StrategyState",
    "TimerEvent",
    "TradeEvent",
]
