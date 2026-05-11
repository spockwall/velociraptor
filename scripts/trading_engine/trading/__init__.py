"""Active trading logic.

  - `observer`: passive multi-exchange orderbook observer (no orders).
  - `strategies/`: per-window strategies driven by the engine. Each strategy
    is one file under `strategies/`; `make_strategy(name, ...)` is the
    factory used by the engine.

Concrete strategies registered today:
  - `probe`     — far-from-touch, no fills expected.
  - `fill_once` — cross the spread, fill once per side per window.
  - `one_shot`  — place once, cancel once, terminate; useful as a single
                  round-trip probe.
"""

from .observer import Observer, TrackedSymbol
from .strategies import (
    FillOnceStrategy,
    OneShotStrategy,
    ProbeStrategy,
    SideState,
    Strategy,
    available_strategies,
    make_strategy,
)

# Back-compat alias: the trading_engine root __init__ used to re-export
# `WindowStrategy`. Anyone importing it gets the abstract base now.
WindowStrategy = Strategy

__all__ = [
    "FillOnceStrategy",
    "Observer",
    "OneShotStrategy",
    "ProbeStrategy",
    "SideState",
    "Strategy",
    "TrackedSymbol",
    "WindowStrategy",
    "available_strategies",
    "make_strategy",
]
