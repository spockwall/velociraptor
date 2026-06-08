"""Strategy registry + factory.

Adding a strategy:
  1. Subclass `Strategy` in a new file under this directory.
  2. Implement `required_topics()` + `setup(dispatcher)`.
  3. Set `name = "..."` on the class (+ `is_window` / `needs_orders` /
     `order_exchange` and a `build` override if it needs CLI args).
  4. Import + register below.

The engine builds the strategy via `build_strategy(name, args, ...)`, which
delegates to each strategy's `build` classmethod. Use `available_strategies()`
for the CLI choice list and `strategy_class(name)` to read class attributes.
"""

from __future__ import annotations

from typing import Type

from ..helpers import (
    MIN_PX,
    PIN_PX,
    SAFE_MID_HIGH,
    SAFE_MID_LOW,
    clamp_px,
    qty_for_notional,
    safe_mid_guard,
)
from .base import Strategy
from .fill_once import FillOnceStrategy
from .kalshi_fill_once import KalshiFillOnceStrategy
from .momentum import MomentumStrategy
from .observe import ObserveStrategy
from .one_shot import OneShotStrategy
from .probe import ProbeStrategy

# Name → class registry. Order here is the order shown in --help.
_REGISTRY: dict[str, Type[Strategy]] = {
    ObserveStrategy.name: ObserveStrategy,
    ProbeStrategy.name: ProbeStrategy,
    FillOnceStrategy.name: FillOnceStrategy,
    OneShotStrategy.name: OneShotStrategy,
    MomentumStrategy.name: MomentumStrategy,
    KalshiFillOnceStrategy.name: KalshiFillOnceStrategy,
}


def available_strategies() -> list[str]:
    """Names of every registered strategy."""
    return list(_REGISTRY.keys())


def strategy_class(name: str) -> Type[Strategy]:
    """Return the registered class for `name` (without instantiating). Lets the
    engine read class attributes — `needs_orders`, `order_exchange` — uniformly
    instead of special-casing strategy names."""
    cls = _REGISTRY.get(name)
    if cls is None:
        raise ValueError(
            f"unknown strategy {name!r}; available: {available_strategies()}"
        )
    return cls


def build_strategy(name: str, args, *, market, router, state) -> Strategy:
    """Construct the strategy registered under `name` from parsed CLI `args`
    plus the shared engine resources. Each strategy's `build` classmethod pulls
    whatever extra knobs it needs from `args`, so the engine never branches on
    strategy name."""
    return strategy_class(name).build(args, market=market, router=router, state=state)


__all__ = [
    "FillOnceStrategy",
    "KalshiFillOnceStrategy",
    "MIN_PX",
    "MomentumStrategy",
    "ObserveStrategy",
    "OneShotStrategy",
    "PIN_PX",
    "ProbeStrategy",
    "SAFE_MID_HIGH",
    "SAFE_MID_LOW",
    "Strategy",
    "available_strategies",
    "build_strategy",
    "clamp_px",
    "qty_for_notional",
    "safe_mid_guard",
    "strategy_class",
]
