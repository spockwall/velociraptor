"""Single-threaded event router.

The Dispatcher is the heart of the engine's runtime. Producers
(`MarketFeed`, `UserFeed`) enqueue immutable `Event`s; this loop pops
them, updates `MarketState`, and fans out to the `Strategy`'s
registered callbacks subject to per-callback throttling.

Why single-threaded:

  - Strategy callbacks read `MarketState` and place orders via
    `OrderRouter`. With a single dispatcher thread the strategy is
    automatically single-threaded — no locks, no reentrancy, no
    "what if a fill arrives in the middle of placing an order".
  - Event order seen by the strategy matches arrival order (FIFO on
    the queue). This makes momentum/stale-quote reasoning tractable.

Throttle semantics:

  - `MarketState` is always updated before the throttle check, so a
    throttled callback's *next* fire sees the freshest data.
  - Throttle key is per (kind, exchange, symbol, callback_id) — each
    registration tracks its own last-fired timestamp. Two strategies
    on the same topic do not interfere.
  - Order / fill / rollover events have no throttle. They're rare and
    each carries action-critical state.

Subscription model:

  - The strategy calls `register_quote(ex, sym, cb, min_interval_ms=...)`
    once per topic it cares about during `Strategy.setup(dispatcher)`.
  - The same (ex, sym) may be registered multiple times with different
    callbacks — useful when one strategy wants both a high-rate
    sampler (for signal) and a low-rate logger.

Shutdown:

  - `stop()` enqueues a `ShutdownEvent`. The loop drains everything
    queued before it, then returns. The strategy keeps receiving
    events up to the sentinel — this matters for fills that arrive
    just before the user hits Ctrl-C.
"""

from __future__ import annotations

import dataclasses
import logging
import queue
import time
from typing import Callable, Optional

from ..market import MarketState
from ..typings.events import (
    Event,
    FillEvent,
    OrderUpdateEvent,
    QuoteEvent,
    RolloverEvent,
    ShutdownEvent,
    SnapshotEvent,
    TradeEvent,
)

log = logging.getLogger(__name__)


QuoteCallback = Callable[..., None]  # signature: (Quote) -> None
SnapshotCallback = Callable[..., None]  # signature: (Snapshot) -> None
TradeCallback = Callable[..., None]  # signature: (Trade) -> None
RolloverCallback = Callable[..., None]  # signature: (full_slug, asset_id) -> None
UserCallback = Callable[..., None]  # signature: (ev_dict) -> None


# Default per-callback throttle for quote/trade registrations that don't
# pass `min_interval_ms`. Set conservatively (2s) so a fast feed
# (Binance bursts at 200/sec) doesn't translate to 200×/sec strategy
# invocations unless the strategy explicitly opts into a tighter cadence
# by passing `min_interval_ms=0` (or a smaller value). MarketState is
# always updated before the throttle check, so dropped events don't
# lose data — `self.market.quote(...)` always returns the latest frame.
DEFAULT_MIN_INTERVAL_MS: float = 2000.0


@dataclasses.dataclass
class _Registration:
    """One callback registered against a topic. `min_interval_ns == 0`
    means fire on every event."""

    callback: Callable[..., None]
    min_interval_ns: int = 0
    last_fired_ns: int = 0  # 0 = never

    def should_fire(self, now_ns: int) -> bool:
        if self.min_interval_ns <= 0:
            return True
        return (now_ns - self.last_fired_ns) >= self.min_interval_ns


class Dispatcher:
    """Single-threaded event router. See module docstring."""

    def __init__(self, q: "queue.Queue[Event]", market: MarketState) -> None:
        self._q = q
        self._market = market
        # Per-topic registrations. Lists so multiple callbacks can
        # subscribe to the same (exchange, symbol).
        self._quote_regs: dict[tuple[str, str], list[_Registration]] = {}
        self._snapshot_regs: dict[tuple[str, str], list[_Registration]] = {}
        self._trade_regs: dict[tuple[str, str], list[_Registration]] = {}
        self._rollover_regs: dict[tuple[str, str], list[_Registration]] = {}
        # User-channel registrations are global (one queue per kind).
        self._order_update_regs: list[_Registration] = []
        self._fill_regs: list[_Registration] = []

    # ── registration (called from Strategy.setup) ──

    def register_quote(
        self,
        exchange: str,
        symbol: str,
        cb: QuoteCallback,
        *,
        min_interval_ms: float = DEFAULT_MIN_INTERVAL_MS,
    ) -> None:
        """Register a quote callback. Default throttle is
        `DEFAULT_MIN_INTERVAL_MS` (2s). Pass `min_interval_ms=0` to
        fire on every quote (e.g. an entry-decision callback that must
        react to every tick)."""
        self._quote_regs.setdefault((exchange, symbol), []).append(
            _Registration(callback=cb, min_interval_ns=int(min_interval_ms * 1e6))
        )

    def register_snapshot(
        self,
        exchange: str,
        symbol: str,
        cb: SnapshotCallback,
        *,
        min_interval_ms: float = DEFAULT_MIN_INTERVAL_MS,
    ) -> None:
        """Register a callback that receives the FULL `Snapshot`
        (bids/asks ladders). Fired off the same publisher frame as
        the BBA-only `QuoteEvent` — register both if you need both,
        otherwise pick the lighter one."""
        self._snapshot_regs.setdefault((exchange, symbol), []).append(
            _Registration(callback=cb, min_interval_ns=int(min_interval_ms * 1e6))
        )

    def register_trade(
        self,
        exchange: str,
        symbol: str,
        cb: TradeCallback,
        *,
        min_interval_ms: float = DEFAULT_MIN_INTERVAL_MS,
    ) -> None:
        """Register a trade callback. Default throttle is
        `DEFAULT_MIN_INTERVAL_MS` (2s). Pass `min_interval_ms=0` to
        fire on every trade print."""
        self._trade_regs.setdefault((exchange, symbol), []).append(
            _Registration(callback=cb, min_interval_ns=int(min_interval_ms * 1e6))
        )

    def register_rollover(
        self,
        exchange: str,
        base_slug: str,
        cb: RolloverCallback,
    ) -> None:
        """Rollovers are rare control-plane events — no throttle option."""
        self._rollover_regs.setdefault((exchange, base_slug), []).append(
            _Registration(callback=cb, min_interval_ns=0)
        )

    def register_order_update(self, cb: UserCallback) -> None:
        self._order_update_regs.append(_Registration(callback=cb))

    def register_fill(self, cb: UserCallback) -> None:
        self._fill_regs.append(_Registration(callback=cb))

    # ── lifecycle ──

    def stop(self) -> None:
        """Enqueue a Shutdown sentinel. Safe to call from any thread."""
        self._q.put(ShutdownEvent())

    def run(self) -> None:
        """Drain the queue until ShutdownEvent is seen. Blocks the
        calling thread — typically the main thread."""
        while True:
            ev = self._q.get()
            try:
                if isinstance(ev, ShutdownEvent):
                    return
                self._dispatch(ev)
            except Exception:  # noqa: BLE001
                # A buggy callback must never take down the dispatcher.
                log.exception(f"dispatcher: callback raised on {type(ev).__name__!r}")

    # ── inner ──

    def _dispatch(self, ev: Event) -> None:
        now_ns = time.monotonic_ns()
        if isinstance(ev, QuoteEvent):
            self._market._update_quote(ev.exchange, ev.symbol, ev.quote)
            self._fire(self._quote_regs.get((ev.exchange, ev.symbol)), now_ns, ev.quote)
        elif isinstance(ev, SnapshotEvent):
            self._market._update_snapshot(ev.exchange, ev.symbol, ev.snapshot)
            self._fire(
                self._snapshot_regs.get((ev.exchange, ev.symbol)),
                now_ns,
                ev.snapshot,
            )
        elif isinstance(ev, TradeEvent):
            self._market._update_trade(ev.exchange, ev.symbol, ev.trade)
            self._fire(self._trade_regs.get((ev.exchange, ev.symbol)), now_ns, ev.trade)
        elif isinstance(ev, RolloverEvent):
            self._market._update_rollover(ev.exchange, ev.base_slug, ev.asset_id)
            self._fire(
                self._rollover_regs.get((ev.exchange, ev.base_slug)),
                now_ns,
                ev.full_slug,
                ev.asset_id,
            )
        elif isinstance(ev, OrderUpdateEvent):
            self._fire(self._order_update_regs, now_ns, ev.ev)
        elif isinstance(ev, FillEvent):
            self._fire(self._fill_regs, now_ns, ev.ev)
        else:
            log.warning(f"dispatcher: unknown event type {type(ev).__name__!r}")

    def _fire(
        self,
        regs: Optional[list[_Registration]],
        now_ns: int,
        *args,
    ) -> None:
        if not regs:
            return
        for reg in regs:
            if not reg.should_fire(now_ns):
                continue
            reg.last_fired_ns = now_ns
            reg.callback(*args)
