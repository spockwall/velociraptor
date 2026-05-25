"""Trading-engine orchestration. Wires feeds, router, MarketState, and
a single Strategy into one Dispatcher loop.

Run as `python -m scripts.trading_engine --strategy <name>`. CLI parsing
lives in `cli.py`; this file owns the run-time class.

**One engine, one strategy.** Multi-market trading is expressed by
launching multiple engine processes. Inside one process the engine
holds:

  - a single `Strategy` instance,
  - a `MarketState` it owns and the dispatcher writes,
  - a `MarketFeed` (ZMQ SUB) that enqueues `QuoteEvent` /
    `TradeEvent` / `RolloverEvent` onto a `queue.Queue`,
  - a `UserFeed` (ZMQ SUB) that enqueues `FillEvent` /
    `OrderUpdateEvent`,
  - a `Dispatcher` that drains the queue on a single thread.

Pre-reqs:
    - zmq_server running (market PUB + user PUB; publishes the static
      rolling topics `polymarket:{base_slug}` and `kalshi:{series}`
      with `full_slug` stamped into the payload).
    - executor running (order ROUTER on :5557) — not required for the
      `observe` strategy.

Bootstrap is purely event-driven: there is no tick loop. The strategy's
`required_topics()` list tells the engine which ZMQ topics to
subscribe before any events flow. Once `Dispatcher.run()` is called
the main thread blocks until a `ShutdownEvent` is enqueued (by a
signal handler or by the strategy itself via `dispatcher.stop()`).
"""

from __future__ import annotations

import argparse
import logging
import os
import queue
import signal
import sys
from pathlib import Path
from typing import Optional

from .cli import parse_args
from .io import OrderRouter, UserFeed
from .io.event_log import EventLog
from .market import MarketFeed, MarketState, Quote, Snapshot, Trade
from .trading import Dispatcher, Strategy, make_strategy
from .typings.events import (
    FillEvent,
    OrderUpdateEvent,
    QuoteEvent,
    RolloverEvent,
    SnapshotEvent,
    TradeEvent,
)
from .typings.state import StrategyState
from .typings.window import PolymarketWindow

log = logging.getLogger(__name__)


# Strategies that need a Polymarket window (and therefore exactly one
# --base-slug). Observer is the exception — it works with N inputs.
_WINDOW_STRATEGIES = {"probe", "fill_once", "one_shot", "momentum"}


def _ensure_writable_dir(path: str) -> None:
    """Create `path` (and parents) and verify the engine can write to
    it. Raises SystemExit with a readable message if not — the engine
    must not start with a log dir it can't actually write."""
    p = Path(path)
    try:
        p.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        raise SystemExit(
            f"--engine-log-dir {path!r}: cannot create directory: {e}. "
            f"Pick a writable path (try --engine-log-dir ~/trading_engine_log)."
        ) from None
    if not os.access(p, os.W_OK):
        raise SystemExit(
            f"--engine-log-dir {path!r}: directory is not writable by this "
            f"process. Fix permissions or pick another path."
        )


class Engine:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        # Engine event log is MANDATORY. The log is the only durable
        # record of what the engine placed / cancelled and what events
        # it saw, so we refuse to start if the directory isn't writable.
        # Daily rotation is built into EventLog itself (see its `_open`).
        _ensure_writable_dir(args.engine_log_dir)
        self.event_log = EventLog(base_dir=args.engine_log_dir, enabled=True)
        self.market = MarketState()
        self.queue: "queue.Queue" = queue.Queue()
        # Producers enqueue Events; the dispatcher drains.
        self.market_feed = MarketFeed(
            args.market_pub,
            args.market_router,
            on_quote=self._enqueue_quote,
            on_trade=self._enqueue_trade,
            on_rollover=self._enqueue_rollover,
            on_snapshot=self._enqueue_snapshot,
        )
        # Observer doesn't need a connected executor or user feed.
        self._needs_orders = args.strategy != "observe"
        self.user_feed = (
            UserFeed(args.user_pub, on_event=self._enqueue_user_event)
            if self._needs_orders
            else None
        )
        self.router = (
            OrderRouter(args.router_endpoint, event_log=self.event_log)
            if self._needs_orders
            else None
        )
        self.dispatcher = Dispatcher(self.queue, self.market)
        self.strategy: Strategy = self._build_strategy()

    # ── build ──

    def _build_strategy(self) -> Strategy:
        name = self.args.strategy
        kwargs: dict = {
            "market": self.market,
            "router": self.router,  # may be None for observer
            "state": StrategyState(),
        }
        if name == "observe":
            kwargs.update(
                binance_symbols=self.args.binance_symbols,
                binance_spot_symbols=self.args.binance_spot_symbols,
                poly_base_slugs=self.args.base_slugs,
                kalshi_series=self.args.kalshi_series,
            )
        elif name in _WINDOW_STRATEGIES:
            slugs = self.args.base_slugs
            if len(slugs) != 1:
                raise SystemExit(
                    f"--strategy {name} requires exactly one --base-slugs "
                    f"value (got {len(slugs)}: {slugs}). Launch multiple "
                    f"engine processes for multiple markets."
                )
            kwargs["window"] = PolymarketWindow(base_slug=slugs[0])
            # Trading-strategy knobs.
            kwargs["safe_mid_low"] = self.args.safe_mid_low
            kwargs["safe_mid_high"] = self.args.safe_mid_high
            if name == "fill_once":
                kwargs["order_notional_usd"] = self.args.order_notional_usd
        return make_strategy(name, **kwargs)

    # ── run ──

    def run(self) -> int:
        self._handle_signals()
        self.market_feed.start()
        self._subscribe_required_topics()
        self.strategy.setup(self.dispatcher)
        if self.user_feed is not None:
            self.user_feed.start()
        if self.router is not None:
            with self.router:
                self._run_dispatcher()
        else:
            self._run_dispatcher()
        return 0

    def _run_dispatcher(self) -> None:
        try:
            self.dispatcher.run()  # blocks until ShutdownEvent
        finally:
            try:
                self.strategy.teardown()
            except Exception:  # noqa: BLE001
                log.exception("strategy.teardown() raised")
            self.market_feed.stop()
            if self.user_feed is not None:
                self.user_feed.stop()
            self.event_log.close()

    def _subscribe_required_topics(self) -> None:
        """Subscribe ZMQ topics declared by the strategy. Rolling topics
        (Polymarket base_slug / Kalshi series) skip the handshake; static
        exchanges (binance / binance_spot / okx) use it."""
        for ex, sym, kind in self.strategy.required_topics():
            handshake = ex not in {"polymarket", "kalshi"}
            if kind == "snapshot":
                self.market_feed.subscribe([sym], exchange=ex, handshake=handshake)
            elif kind == "trade":
                self.market_feed.subscribe_trades([sym], exchange=ex)
            else:
                log.warning(
                    f"required_topics: unknown kind {kind!r} for {ex}:{sym} — skipped"
                )
            log.info(f"subscribed {ex}:{sym} ({kind})")

    # ── producer hooks (run on feed threads) ──

    def _enqueue_quote(self, exchange: str, symbol: str, q: Quote) -> None:
        self.queue.put(QuoteEvent(exchange=exchange, symbol=symbol, quote=q))

    def _enqueue_snapshot(self, exchange: str, symbol: str, s: Snapshot) -> None:
        self.queue.put(SnapshotEvent(exchange=exchange, symbol=symbol, snapshot=s))

    def _enqueue_trade(self, exchange: str, symbol: str, t: Trade) -> None:
        self.queue.put(TradeEvent(exchange=exchange, symbol=symbol, trade=t))

    def _enqueue_rollover(
        self, exchange: str, base_slug: str, full_slug: str, asset_id: str
    ) -> None:
        self.queue.put(
            RolloverEvent(
                exchange=exchange,
                base_slug=base_slug,
                full_slug=full_slug,
                asset_id=asset_id,
            )
        )

    def _enqueue_user_event(self, topic: str, ev: dict) -> None:
        # Record every received event for research, even if the
        # strategy doesn't claim it. `record_event` no-ops when the log
        # is disabled.
        self.event_log.record_event(topic, ev)
        kind = ev.get("type")
        if kind == "fill":
            self.queue.put(FillEvent(topic=topic, ev=ev))
        elif kind == "order_update":
            self.queue.put(OrderUpdateEvent(topic=topic, ev=ev))

    # ── signals ──

    def _handle_signals(self) -> None:
        def _handler(_sig: int, _frame: object) -> None:
            log.info("shutdown signal received")
            self.dispatcher.stop()

        signal.signal(signal.SIGINT, _handler)
        signal.signal(signal.SIGTERM, _handler)


# ── main ─────────────────────────────────────────────────────────────────────


def main() -> int:
    args = parse_args()
    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s.%(msecs)03d %(levelname)-5s %(name)s — %(message)s",
        datefmt="%H:%M:%S",
    )
    log.info(
        f"starting engine strategy={args.strategy} slugs={args.base_slugs} notional=${args.order_notional_usd:.2f}"
    )
    eng = Engine(args)
    try:
        return eng.run()
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
