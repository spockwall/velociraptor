"""Event types carried on the engine's queue.

Pure data — immutable dataclasses, no behavior. The consumer side
(`Dispatcher`) lives in `trading/dispatcher.py`; keeping the types here
lets producers, the dispatcher, and tests share one definition without
importing the threading machinery.

Producers and the event each type represents:
  - `MarketFeed`  → `QuoteEvent` / `TradeEvent` / `RolloverEvent`
  - `UserFeed`    → `FillEvent` / `OrderUpdateEvent`
  - shutdown      → `ShutdownEvent` (enqueued last; drains the rest first)

There is intentionally **no `TimerEvent`** — the engine is fully
event-driven; periodic logic must be expressed by listening to a real
data event (typically a quote on the relevant topic).
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
class RolloverEvent:
    """Fired by `MarketFeed` when a rolling topic's `full_slug` differs
    from the last seen value for (exchange, base_slug). For Polymarket
    `base_slug` is the topic id; for Kalshi it's the series."""

    exchange: str
    base_slug: str
    full_slug: str
    asset_id: str


@dataclasses.dataclass(frozen=True)
class FillEvent:
    topic: str
    ev: dict


@dataclasses.dataclass(frozen=True)
class OrderUpdateEvent:
    topic: str
    ev: dict


@dataclasses.dataclass(frozen=True)
class ShutdownEvent:
    """Sentinel. Enqueued last so the dispatcher drains everything before
    it, then returns."""


Event = Union[
    QuoteEvent, TradeEvent, RolloverEvent, FillEvent, OrderUpdateEvent, ShutdownEvent
]
