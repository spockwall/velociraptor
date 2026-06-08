"""Active trading logic.

  - `strategies/`: per-strategy event-driven logic. Each strategy lives
    in its own file; `build_strategy(name, args, ...)` is the factory the
    engine uses to build the single Strategy instance per run (it delegates
    to each strategy's `build` classmethod).
  - `dispatcher.py`: single-threaded event router that consumes the
    engine queue and fans events into the strategy's registered
    callbacks.

Concrete strategies registered today:
  - `observe`   — passive multi-exchange watcher (no orders).
  - `probe`     — place + cancel at the price floor on every quote.
  - `fill_once` — cross the spread, fill once per window.
  - `one_shot`  — place once, cancel once, terminate.
  - `momentum`  — Binance signal → one small Polymarket position with
                  strategy-managed TP/SL.
"""

from .dispatcher import Dispatcher
from .strategies import (
    FillOnceStrategy,
    KalshiFillOnceStrategy,
    MomentumStrategy,
    ObserveStrategy,
    OneShotStrategy,
    ProbeStrategy,
    Strategy,
    available_strategies,
    build_strategy,
    strategy_class,
)

# Back-compat alias for older imports.
WindowStrategy = Strategy

__all__ = [
    "Dispatcher",
    "FillOnceStrategy",
    "KalshiFillOnceStrategy",
    "MomentumStrategy",
    "ObserveStrategy",
    "OneShotStrategy",
    "ProbeStrategy",
    "Strategy",
    "WindowStrategy",
    "available_strategies",
    "build_strategy",
    "strategy_class",
]
