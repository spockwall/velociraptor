"""Strategy registry + factory.

Adding a strategy:
  1. Subclass `Strategy` in a new file under this directory.
  2. Implement `required_topics()` + `setup(dispatcher)`.
  3. Set `name = "..."` on the class.
  4. Import + register below.

The engine resolves the strategy by name via
`make_strategy(name, **kwargs)`. Use `available_strategies()` for the
CLI choice list.
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
}


def available_strategies() -> list[str]:
    """Names of every registered strategy."""
    return list(_REGISTRY.keys())


def make_strategy(name: str, **kwargs) -> Strategy:
    """Instantiate the strategy registered under `name`. Extra kwargs
    are forwarded to the concrete class — see each strategy file for
    its specific constructor knobs."""
    cls = _REGISTRY.get(name)
    if cls is None:
        raise ValueError(
            f"unknown strategy {name!r}; available: {available_strategies()}"
        )
    return cls(**kwargs)


__all__ = [
    "FillOnceStrategy",
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
    "clamp_px",
    "make_strategy",
    "qty_for_notional",
    "safe_mid_guard",
]
