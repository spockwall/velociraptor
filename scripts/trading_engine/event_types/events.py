"""Event types carried on the engine's queue.

Pure data — immutable dataclasses, no behavior. The consumer side
(`Dispatcher`, `TimerThread`) lives in `trading/dispatcher.py`; keeping the
types here lets producers, the dispatcher, and tests share one definition
without importing the threading machinery.

Producers and the event each type represents:
  - `MarketFeed`  → `QuoteEvent` / `TradeEvent`
  - `UserFeed`    → `FillEvent` / `OrderUpdateEvent`
  - `TimerThread` → `TimerEvent`
  - shutdown      → `ShutdownEvent` (enqueued last; drains the rest first)
"""

from __future__ import annotations

import dataclasses
from typing import Union

from ..market import Quote, Trade


@dataclasses.dataclass(frozen=True)
class QuoteEvent:
    exchange: str
    symbol: str
    quote: Quote


@dataclasses.dataclass(frozen=True)
class TradeEvent:
    exchange: str
    symbol: str
    trade: Trade


@dataclasses.dataclass(frozen=True)
class FillEvent:
    topic: str
    ev: dict


@dataclasses.dataclass(frozen=True)
class OrderUpdateEvent:
    topic: str
    ev: dict


@dataclasses.dataclass(frozen=True)
class TimerEvent:
    now_monotonic: float
    now_wall: float  # time.time(); compared against window_start/end


@dataclasses.dataclass(frozen=True)
class ShutdownEvent:
    """Sentinel. Enqueued last so the dispatcher drains everything before
    it, then returns."""


Event = Union[
    QuoteEvent,
    TradeEvent,
    FillEvent,
    OrderUpdateEvent,
    TimerEvent,
    ShutdownEvent,
]
