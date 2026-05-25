"""Event types carried on the engine's queue.

Pure data — immutable dataclasses, no behavior. The consumer side
(`Dispatcher`) lives in `trading/dispatcher.py`; keeping the types here
lets producers, the dispatcher, and tests share one definition without
importing the threading machinery.

Producers and the event each type represents:
  - `Engine`      → `BootstrapEvent` (once, at startup, for
                    window-strategies)
  - `MarketFeed`  → `QuoteEvent` / `SnapshotEvent` / `TradeEvent` /
                    `RolloverEvent`
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
class BootstrapEvent:
    """Fired once by the engine at process start for every
    window-based strategy. Carries the current window's `full_slug`
    (computed from base_slug + clock — see `market.gamma.current_full_slug`
    + `interval_secs_from_base_slug`) so the dispatcher can resolve
    asset_ids via `MarketState.on_rollover` BEFORE any quote arrives.

    Distinct from `RolloverEvent` on purpose: bootstrap is a
    process-lifecycle event with a different trigger (no prior frame,
    slug derived from the clock); the dispatcher does NOT fire the
    strategy's `_on_rollover` callback on bootstrap. Strategies see
    asset_ids materialised on first `_on_quote` instead."""

    exchange: str
    base_slug: str
    full_slug: str


@dataclasses.dataclass(frozen=True)
class RolloverEvent:
    """Fired by `MarketFeed` when the payload `full_slug` field on a
    rolling topic differs from the last seen value. Carries
    `(exchange, base_slug, full_slug)`; per-token asset_ids are
    resolved by `MarketState.on_rollover` (Gamma) before strategy
    `_on_rollover` callbacks fire, then read via
    `MarketState.asset_ids`."""

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
    BootstrapEvent,
    QuoteEvent,
    SnapshotEvent,
    TradeEvent,
    RolloverEvent,
    FillEvent,
    OrderUpdateEvent,
    ShutdownEvent,
]
