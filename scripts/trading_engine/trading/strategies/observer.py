"""Observer — passive multi-exchange orderbook watcher.

A regular `Strategy` in the new event-driven model. Subscribes to a
configurable set of (exchange, symbol) snapshot + trade streams,
registers a callback per stream that logs a single per-stream status
line, and registers a separate "summary" callback on one of the
Polymarket streams that periodically dumps a unified table.

Never places or cancels orders. The contract:

  1. Every configured stream produces a status update at most every
     `per_stream_min_secs` seconds (avoids drowning the log on fast
     feeds while still proving "the feed is alive").
  2. A unified table dumps at most every `table_min_secs` seconds,
     piggy-backing on whichever Polymarket / Kalshi quote happens to
     arrive after the throttle expires (or the first quote we see at
     all, when nothing more specific is configured).

This replaces the old standalone `trading/observer.py` and runs
through the same `Dispatcher` as the trading strategies.
"""

from __future__ import annotations

import logging
import time
from typing import Optional

from ...market import Quote, Trade
from ..dispatcher import Dispatcher
from .base import Strategy

log = logging.getLogger(__name__)


# Tunables — sensible defaults; not exposed to CLI.
_PER_STREAM_MIN_MS = 5_000.0  # log a "tick" line per stream at most every 5s
_TABLE_MIN_MS = 5_000.0  # dump the unified table at most every 5s


class ObserverStrategy(Strategy):
    name = "observe"

    def __init__(
        self,
        *,
        binance_symbols: Optional[list[str]] = None,
        binance_spot_symbols: Optional[list[str]] = None,
        poly_base_slugs: Optional[list[str]] = None,
        kalshi_series: Optional[list[str]] = None,
        **kwargs,
    ):
        # Observer doesn't use `window` even though the base accepts it.
        super().__init__(**kwargs)
        self.binance_symbols = list(binance_symbols or [])
        self.binance_spot_symbols = list(binance_spot_symbols or [])
        self.poly_base_slugs = list(poly_base_slugs or [])
        self.kalshi_series = list(kalshi_series or [])

    # ── data dependencies ──

    def required_topics(self) -> list[tuple[str, str, str]]:
        topics: list[tuple[str, str, str]] = []
        for sym in self.binance_symbols:
            topics.append(("binance", sym, "snapshot"))
            topics.append(("binance", sym, "trade"))
        for sym in self.binance_spot_symbols:
            topics.append(("binance_spot", sym, "snapshot"))
            topics.append(("binance_spot", sym, "trade"))
        for slug in self.poly_base_slugs:
            topics.append(("polymarket", slug, "snapshot"))
            topics.append(("polymarket", slug, "trade"))
        for series in self.kalshi_series:
            topics.append(("kalshi", series, "snapshot"))
            topics.append(("kalshi", series, "trade"))
        return topics

    # ── event registration ──

    def setup(self, dispatcher: Dispatcher) -> None:
        for ex, sym, kind in self.required_topics():
            if kind == "snapshot":
                dispatcher.register_quote(
                    ex,
                    sym,
                    self._make_quote_cb(ex, sym),
                    min_interval_ms=_PER_STREAM_MIN_MS,
                )
            else:
                dispatcher.register_trade(
                    ex,
                    sym,
                    self._make_trade_cb(ex, sym),
                    min_interval_ms=_PER_STREAM_MIN_MS,
                )

        # The unified table piggybacks on whichever stream we see most
        # often. Register the table dump on every snapshot stream we
        # subscribed to; each invocation is itself throttled by a
        # shared deadline so we still dump at most once per window.
        self._table_last_ms = 0.0
        for ex, sym, kind in self.required_topics():
            if kind != "snapshot":
                continue
            dispatcher.register_quote(
                ex,
                sym,
                self._on_table_tick,
                # no per-call throttle here; the shared deadline limits it.
                min_interval_ms=0.0,
            )

        if not self.required_topics():
            log.warning(
                "observer: no streams configured — pass --binance-symbols / "
                "--base-slugs / --kalshi-series / --binance-spot-symbols"
            )

    # ── callbacks ──

    def _make_quote_cb(self, exchange: str, symbol: str):
        def _cb(q: Quote) -> None:
            log.info(
                f"[{exchange}:{symbol}] bid={_fp(q.best_bid)} ask={_fp(q.best_ask)} "
                f"mid={_fp(q.mid)} full_slug={q.full_slug or '—'} seq={q.sequence}"
            )

        return _cb

    def _make_trade_cb(self, exchange: str, symbol: str):
        def _cb(t: Trade) -> None:
            log.info(
                f"[{exchange}:{symbol}] trade px={_fp(t.price)} size={_fp(t.size)} "
                f"side={t.side} full_slug={t.full_slug or '—'}"
            )

        return _cb

    def _on_table_tick(self, _q: Quote) -> None:
        """Dump the unified state table at most every `_TABLE_MIN_MS`."""
        now_ms = time.monotonic() * 1000.0
        if now_ms - self._table_last_ms < _TABLE_MIN_MS:
            return
        self._table_last_ms = now_ms

        quotes = self.market.snapshot_all()
        trades = self.market.trades_all()
        if not quotes and not trades:
            return

        log.info("─── observer snapshot ─────────────────────────────────────")
        keys = sorted(set(quotes) | set(trades))
        last_exchange: Optional[str] = None
        for key in keys:
            exchange, symbol = key
            if exchange != last_exchange:
                log.info(f"[{exchange}]")
                last_exchange = exchange
            q = quotes.get(key)
            t = trades.get(key)
            log.info(f"  {_format_row(symbol, q, t)}")


# ── helpers ──────────────────────────────────────────────────────────────────


def _fp(v: Optional[float]) -> str:
    return f"{v:.6f}" if isinstance(v, (int, float)) else "—"


def _format_row(symbol: str, q: Optional[Quote], t: Optional[Trade]) -> str:
    now_ns = time.time_ns()
    if q is None:
        quote_part = f"{symbol:32s} (no quote)"
    else:
        age_ms = (now_ns - q.received_ns) / 1e6
        quote_part = (
            f"{symbol:32s}  bid={_fp(q.best_bid):>10s}  ask={_fp(q.best_ask):>10s}  "
            f"mid={_fp(q.mid):>10s}  age={age_ms:7.0f}ms"
        )
    if t is None:
        trade_part = "  last=—"
    else:
        t_age_ms = (now_ns - t.received_ns) / 1e6
        trade_part = (
            f"  last={_fp(t.price):>10s}x{t.size:<8.4g}{t.side:<4s} "
            f"({t_age_ms:6.0f}ms)"
        )
    return quote_part + trade_part
