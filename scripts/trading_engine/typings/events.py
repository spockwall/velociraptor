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

from ..market import Quote, Snapshot, Trade


@dataclasses.dataclass(frozen=True)
class QuoteEvent:
    exchange: str
    symbol: str
    quote: Quote


@dataclasses.dataclass(frozen=True)
class SnapshotEvent:
    """Full orderbook snapshot (depth). Fired off the same publisher
    frame as `QuoteEvent`; consumers that don't need depth ignore
    this and register against `QuoteEvent` instead."""

    exchange: str
    symbol: str
    snapshot: Snapshot


@dataclasses.dataclass(frozen=True)
class TradeEvent:
    exchange: str
    symbol: str
    trade: Trade


@dataclasses.dataclass(frozen=True)
class RolloverEvent:
    """Fired by `MarketFeed` when the payload `full_slug` field on a
    rolling topic differs from the last seen value, and also synthesised
    once by the engine at startup so the bootstrap path goes through
    the same dispatcher code. Carries `(exchange, base_slug,
    full_slug)`; per-token asset_ids are resolved by
    `MarketState.on_rollover` (Gamma) before strategy callbacks
    fire, then read via `MarketState.asset_ids`."""

    exchange: str
    base_slug: str
    full_slug: str


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
    QuoteEvent,
    SnapshotEvent,
    TradeEvent,
    RolloverEvent,
    FillEvent,
    OrderUpdateEvent,
    ShutdownEvent,
]
