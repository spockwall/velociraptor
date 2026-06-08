"""Strategy base class — event-driven API.

A `Strategy` is a single instance per `Engine` run. It declares which
(exchange, symbol) market-data streams it wants and which callbacks
should fire when each event arrives. The engine wires the producers
(`MarketFeed`, `UserFeed`) to a `Dispatcher`, then calls
`Strategy.setup(dispatcher)` once at startup; from that point on the
strategy reacts to events.

Lifecycle:

  - `required_topics()` — declarative list of (exchange, symbol, kind)
    the engine subscribes BEFORE setup() runs. `kind` is "snapshot" or
    "trade". This is the single source of truth for the strategy's
    data dependencies; if a topic isn't here, no events arrive.
  - `setup(dispatcher)` — register callbacks. Typical body:
        dispatcher.register_quote("polymarket", self.window.base_slug,
            self._on_poly_quote)
        dispatcher.register_fill(self._on_fill)
  - `teardown()` — called on shutdown after the dispatcher stops.
    Cancel any live orders here.

No `tick()`, no `_target_px`, no polling — this is intentional. If you
need periodic logic ("place a fresh order every N seconds"), drive it
from a quote callback that already fires on the relevant topic.

Helpers (clamp_px, qty_for_notional, MIN_PX, safe_mid_guard, …) live
in `helpers.py`; strategies import what they need.

Concrete strategies registered today: observe, probe, fill_once,
one_shot, momentum.
"""

from __future__ import annotations

import abc
import logging
from typing import Literal, Optional

from ...io import OrderRouter
from ...market import MarketState
from ...typings.state import StrategyState
from ...typings.window import PolymarketWindow as MarketWindow
from ..dispatcher import Dispatcher

log = logging.getLogger(__name__)


class Strategy(abc.ABC):
    """Abstract base for every strategy. See module docstring."""

    #: Stable name used by the registry / CLI. Subclasses must override.
    name: str = "abstract"

    #: Whether the engine should connect an `OrderRouter` + `UserFeed` for this
    #: strategy. Pure watchers (observe) override to `False` so the engine runs
    #: without an executor connection. The engine reads this off the CLASS — it
    #: never special-cases strategy names.
    needs_orders: bool = True

    #: Which exchange the strategy's orders route to (the `OrderRouter` is built
    #: with this). Override per venue (e.g. a Kalshi strategy → `"kalshi"`).
    order_exchange: str = "polymarket"

    #: Whether this is a per-Polymarket-window strategy: it trades exactly one
    #: rolling market (`--base-slugs` of length 1) and is constructed with a
    #: `PolymarketWindow` + the safe-mid knobs. The default `build` handles this
    #: when `True`. Non-window strategies (observe, kalshi_*) leave it `False`.
    #: The engine reads this off the CLASS — it keeps no strategy-name list.
    is_window: bool = False

    @classmethod
    def build(
        cls,
        args,
        *,
        market: MarketState,
        router: Optional[OrderRouter],
        state: StrategyState,
    ) -> "Strategy":
        """Construct the strategy from parsed CLI `args` + the shared engine
        resources. This is the single, uniform construction surface the engine
        calls — each strategy pulls whatever extra knobs it needs from `args`
        here, so the engine never branches on strategy name.

        Default behaviour:
          - `is_window` strategies get a `PolymarketWindow` (from the single
            `--base-slugs` value) + the safe-mid knobs via [`window_kwargs`].
          - everything else gets the bare `(market, router, state)`.

        Strategies with extra constructor params override this and call
        `cls.window_kwargs(args, ...)` (window) or build kwargs themselves.
        """
        if cls.is_window:
            return cls(**cls.window_kwargs(args, market=market, router=router, state=state))
        return cls(market=market, router=router, state=state)

    @staticmethod
    def _window_from_args(args) -> MarketWindow:
        """Validate that exactly one `--base-slugs` was given and build the
        `PolymarketWindow`. Window strategies trade one rolling market per
        engine process."""
        slugs = args.base_slugs
        if len(slugs) != 1:
            raise SystemExit(
                f"--strategy {args.strategy} requires exactly one --base-slugs "
                f"value (got {len(slugs)}: {slugs}). Launch multiple engine "
                f"processes for multiple markets."
            )
        return MarketWindow(base_slug=slugs[0])

    @classmethod
    def window_kwargs(
        cls,
        args,
        *,
        market: MarketState,
        router: Optional[OrderRouter],
        state: StrategyState,
    ) -> dict:
        """Shared constructor kwargs for a window strategy that takes the
        safe-mid knobs (probe / one_shot / fill_once). Strategies with extra
        params spread this and add their own (e.g. `fill_once` adds
        `order_notional_usd`). Window strategies WITHOUT safe-mid (momentum)
        build with just `_window_from_args` instead."""
        return {
            "market": market,
            "router": router,
            "state": state,
            "window": cls._window_from_args(args),
            "safe_mid_low": args.safe_mid_low,
            "safe_mid_high": args.safe_mid_high,
        }

    def __init__(
        self,
        *,
        market: MarketState,
        router: OrderRouter,
        state: StrategyState,
        window: Optional[MarketWindow] = None,
    ):
        self.market = market
        self.router = router
        self.state = state
        # `window` is only set for per-Polymarket-window strategies
        # (probe / fill_once / one_shot / momentum). Observe leaves it
        # None and walks topics it owns directly.
        self.window = window

    # ── label / attribution helpers ──

    @property
    def label(self) -> str:
        """Human-readable tag used in log lines and order attribution."""
        if self.window is None:
            return self.name
        return self.window.full_slug or self.window.base_slug

    def _attribution(self) -> dict:
        """Kwargs passed to OrderRouter.place_limit / cancel so the
        engine's event log captures which strategy fired the action."""
        return {"strategy": self.name, "label": self.label}

    # ── extension points ──

    def required_topics(self) -> list[tuple[str, str, str]]:
        """Return a list of `(exchange, symbol, kind)` tuples the engine
        should subscribe BEFORE `setup()` is called. `kind` is
        "snapshot" (the depth/BBA stream) or "trade" (the last-trade
        stream).

        Default: nothing. Strategies that don't override get no data.
        """
        return []

    @abc.abstractmethod
    def setup(self, dispatcher: Dispatcher) -> None:
        """Register every callback the strategy needs. Called once at
        startup, after `required_topics()` have been subscribed and
        before any events flow."""

    def teardown(self) -> None:
        """Called on shutdown. Cancel any live orders. Default no-op."""

    # ── asset_id reader (Polymarket rolling windows) ──

    def _asset_id(self, side: Literal["up", "down"]) -> Optional[str]:
        """Return the asset_id for the current window's `side`
        (`"up"` = YES token, `"down"` = NO). Returns `None` if:
          - no window is set,
          - no rollover has been seen yet,
          - or Gamma resolve failed (stored as `None` by
            `MarketState.on_rollover`).

        Strategies MUST guard on `None` and refuse to trade.

        The dispatcher refreshes the `(full_slug → (up, down))` map on
        every `RolloverEvent` (real or synthetic) via
        `MarketState.on_rollover`, so the strategy never calls Gamma
        directly.
        """
        if self.window is None or self.window.full_slug is None:
            return None
        entry = self.market.asset_ids(self.window.full_slug)
        if entry is None:
            return None
        up, down = entry
        return up if side == "up" else down
