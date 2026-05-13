"""Trading-engine orchestration. Wires market discovery, feeds, router, and
per-window strategies into a single tick loop.

Run as `python -m scripts.trading_engine --step <N>`. CLI parsing lives in
`cli.py`; this file owns the run-time class.

Pre-reqs:
    - backend running on :3000 (for market discovery)
    - zmq_server running       (market PUB + user PUB)
    - executor running         (order ROUTER on :5557)

The engine refreshes the active-window list every `--rediscover-secs`
seconds, instantiates a `WindowStrategy` per new window, and cancels all
known orders + drops the strategy when a window expires.
"""

from __future__ import annotations

import argparse
import logging
import signal
import sys
import threading
import time

from .cli import parse_args
from .io import OrderRouter, UserFeed
from .io.event_log import EventLog
from .market import MarketFeed, MarketWindow, discover
from .trading import Observer, Strategy, make_strategy

log = logging.getLogger(__name__)


class Engine:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.strategies: dict[str, Strategy] = {}  # keyed by full_slug
        # Engine-side append-only log. `--no-engine-log` makes every record
        # a no-op (constructor still returns a sentinel object).
        self.event_log = EventLog(
            base_dir=None if getattr(args, "no_engine_log", False) else args.engine_log_dir,
            enabled=not getattr(args, "no_engine_log", False),
        )
        self.market_feed = MarketFeed(args.market_pub, args.market_router)
        self.user_feed = UserFeed(args.user_pub, on_event=self._on_user_event)
        self.router = OrderRouter(args.router_endpoint, event_log=self.event_log)
        self._stop = threading.Event()
        self._lock = threading.Lock()

    def run(self) -> int:
        self._handle_signals()
        self.market_feed.start()
        # `observe` never sends orders — skip the user feed + executor
        # connection entirely so the observer can run against a stack
        # with no executor.
        if self.args.strategy == "observe":
            try:
                obs = Observer(
                    feed=self.market_feed,
                    backend_url=self.args.backend_url,
                    binance_symbols=self.args.binance_symbols,
                    binance_spot_symbols=self.args.binance_spot_symbols,
                    poly_base_slugs=self.args.base_slugs,
                    kalshi_series=self.args.kalshi_series,
                    report_interval_secs=self.args.report_secs,
                    rediscover_interval_secs=self.args.rediscover_secs,
                )
                # Hook so SIGINT propagates from the parent thread.
                self._observer = obs
                return obs.run()
            finally:
                self.market_feed.stop()
                self.event_log.close()

        # Trading strategies (probe / fill_once / one_shot / ...).
        self.user_feed.start()
        with self.router:
            self._rediscover()
            last_rediscover = time.monotonic()
            try:
                while not self._stop.is_set():
                    now = time.monotonic()
                    if now - last_rediscover >= self.args.rediscover_secs:
                        self._rediscover()
                        last_rediscover = now
                    self._tick()
                    # one_shot terminates when every tracked side has run
                    # its place+cancel once.
                    if self._all_strategies_done():
                        log.info("all strategies done — exiting")
                        break
                    self._stop.wait(timeout=self.args.tick_secs)
            finally:
                self._shutdown_orders()
        self.market_feed.stop()
        self.user_feed.stop()
        self.event_log.close()
        return 0

    def _all_strategies_done(self) -> bool:
        """True when every strategy reports `is_done` (`OneShotStrategy`).
        Strategies without an `is_done` attribute are treated as never-done
        — so probe / fill_once never trigger this branch."""
        with self._lock:
            strats = list(self.strategies.values())
        if not strats:
            return False
        return all(getattr(s, "is_done", False) for s in strats)

    # ── discovery + lifecycle ──

    def _rediscover(self) -> None:
        try:
            windows = discover(self.args.base_slugs, self.args.backend_url)
        except Exception as e:  # noqa: BLE001
            log.warning("rediscover failed: %s", e)
            return

        seen: set[str] = set()
        for w in windows:
            seen.add(w.full_slug)
            if w.full_slug not in self.strategies:
                self._add_window(w)

        # Drop windows that are no longer in discovery output.
        with self._lock:
            for stale_slug in list(self.strategies.keys()):
                if stale_slug not in seen:
                    log.info("window expired: %s — cancelling orders", stale_slug)
                    try:
                        self.strategies[stale_slug].cancel_all()
                    except Exception:  # noqa: BLE001
                        log.exception("cancel_all on expiring window")
                    del self.strategies[stale_slug]

    def _add_window(self, w: MarketWindow) -> None:
        log.info(
            "tracking new window %s  (up=%s no=%s, ends in %ds)",
            w.full_slug,
            w.up_asset_id[:8],
            w.down_asset_id[:8],
            w.window_end - int(time.time()),
        )
        # Subscribe the market feed BEFORE the first tick so the first quote
        # has a chance to arrive.
        self.market_feed.subscribe([w.up_asset_id, w.down_asset_id])
        strat = make_strategy(
            self.args.strategy,
            window=w,
            router=self.router,
            feed=self.market_feed,
            order_notional_usd=self.args.order_notional_usd,
            safe_mid_low=self.args.safe_mid_low,
            safe_mid_high=self.args.safe_mid_high,
        )
        with self._lock:
            self.strategies[w.full_slug] = strat

    # ── tick ──

    def _tick(self) -> None:
        with self._lock:
            strats = list(self.strategies.values())
        for s in strats:
            try:
                s.tick()
            except Exception:  # noqa: BLE001
                log.exception("tick failed for %s", s.label)

    # ── user events ──

    def _on_user_event(self, topic: str, ev: dict) -> None:
        # Record every received event first — even if no strategy claims it,
        # research wants the full stream. `record_event` is a no-op when
        # the log is disabled.
        self.event_log.record_event(topic, ev)

        kind = ev.get("type")
        with self._lock:
            strats = list(self.strategies.values())
        if kind == "order_update":
            for s in strats:
                s.on_order_update(ev)
        elif kind == "fill":
            for s in strats:
                s.on_fill(ev)

    # ── shutdown ──

    def _handle_signals(self) -> None:
        def _handler(_sig: int, _frame: object) -> None:
            log.info("shutdown signal received")
            self._stop.set()
            obs = getattr(self, "_observer", None)
            if obs is not None:
                obs.request_stop()

        signal.signal(signal.SIGINT, _handler)
        signal.signal(signal.SIGTERM, _handler)

    def _shutdown_orders(self) -> None:
        log.info("cancelling all known orders before exit")
        with self._lock:
            strats = list(self.strategies.values())
        for s in strats:
            try:
                s.cancel_all()
            except Exception:  # noqa: BLE001
                log.exception("shutdown cancel failed for %s", s.label)


# ── main ─────────────────────────────────────────────────────────────────────


def main() -> int:
    args = parse_args()
    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s.%(msecs)03d %(levelname)-5s %(name)s — %(message)s",
        datefmt="%H:%M:%S",
    )
    log.info(
        "starting engine strategy=%s slugs=%s notional=$%.2f",
        args.strategy,
        args.base_slugs,
        args.order_notional_usd,
    )
    eng = Engine(args)
    try:
        return eng.run()
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
