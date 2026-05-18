"""Step 0 — passive orderbook observer. Never places or cancels an order.

Subscribes to Binance, Polymarket, and Kalshi via the zmq_server market PUB
and periodically logs a status line per (exchange, symbol). Used to
verify:

  1. The feed is connecting and decoding snapshots correctly.
  2. Polymarket / Kalshi rolling-window rotation happens smoothly — when a
     new full_slug / ticker shows up in discovery, we subscribe it and
     drop the old one without missing a tick.
  3. The three exchange families produce the BBA we expect.
"""

from __future__ import annotations

import dataclasses
import logging
import threading
import time
from typing import Optional

from ..market import (
    KalshiWindow,
    MarketFeed,
    MarketWindow,
    Quote,
    Trade,
    discover,
    discover_kalshi,
)

log = logging.getLogger(__name__)


# ── Tracker — what we're currently subscribed to ────────────────────────────


@dataclasses.dataclass
class TrackedSymbol:
    exchange: str
    symbol: str
    label: str          # human-friendly tag for log output
    rotates: bool       # True for Polymarket/Kalshi windowed markets
    window_end: Optional[int] = None   # only set when rotates=True


class Observer:
    def __init__(
        self,
        feed: MarketFeed,
        backend_url: str,
        binance_symbols: list[str],
        binance_spot_symbols: list[str],
        poly_base_slugs: list[str],
        kalshi_series: list[str],
        report_interval_secs: float = 5.0,
        rediscover_interval_secs: float = 30.0,
    ):
        self.feed = feed
        self.backend_url = backend_url
        self.binance_symbols = binance_symbols
        self.binance_spot_symbols = binance_spot_symbols
        self.poly_base_slugs = poly_base_slugs
        self.kalshi_series = kalshi_series
        self.report_interval = report_interval_secs
        self.rediscover_interval = rediscover_interval_secs

        # `tracked` is keyed by topic string "{exchange}:{symbol}".
        self._tracked: dict[str, TrackedSymbol] = {}
        self._stop = threading.Event()

    # ── lifecycle ──

    def run(self) -> int:
        self._subscribe_static()
        last_rediscover = 0.0
        try:
            while not self._stop.is_set():
                now = time.monotonic()
                if now - last_rediscover >= self.rediscover_interval:
                    self._rediscover()
                    last_rediscover = now
                self._report()
                self._stop.wait(timeout=self.report_interval)
        except KeyboardInterrupt:
            pass
        return 0

    def request_stop(self) -> None:
        self._stop.set()

    # ── subscribers ──

    def _subscribe_static(self) -> None:
        """Binance + Binance Spot symbols are config-time fixed."""
        for sym in self.binance_symbols:
            self._track("binance", sym, label=f"binance:{sym}", rotates=False)
        for sym in self.binance_spot_symbols:
            self._track(
                "binance_spot", sym, label=f"binance_spot:{sym}", rotates=False
            )

    def _rediscover(self) -> None:
        """Poll the backend for Polymarket + Kalshi active windows. Subscribe
        new ones, unsubscribe expired ones."""

        # ── Polymarket ──
        try:
            poly_windows = discover(self.poly_base_slugs, self.backend_url)
        except Exception as e:  # noqa: BLE001
            log.warning("poly discover failed: %s", e)
            poly_windows = []
        active_poly: set[str] = set()
        for w in poly_windows:
            for side_name, asset_id in (
                ("up", w.up_asset_id),
                ("down", w.down_asset_id),
            ):
                active_poly.add(asset_id)
                if f"polymarket:{asset_id}" in self._tracked:
                    continue
                self._track(
                    "polymarket",
                    asset_id,
                    label=f"{w.full_slug}/{side_name}",
                    rotates=True,
                    window_end=w.window_end,
                )

        # ── Kalshi ──
        try:
            kal_windows = discover_kalshi(self.kalshi_series, self.backend_url)
        except Exception as e:  # noqa: BLE001
            log.warning("kalshi discover failed: %s", e)
            kal_windows = []
        active_kal: set[str] = set()
        for k in kal_windows:
            active_kal.add(k.ticker)
            if f"kalshi:{k.ticker}" in self._tracked:
                continue
            self._track(
                "kalshi",
                k.ticker,
                label=f"{k.series}/{k.ticker}",
                rotates=True,
                window_end=k.window_end,
            )

        # ── drop expired rotating subs ──
        stale: list[TrackedSymbol] = []
        for ts in list(self._tracked.values()):
            if not ts.rotates:
                continue
            if ts.exchange == "polymarket" and ts.symbol not in active_poly:
                stale.append(ts)
            elif ts.exchange == "kalshi" and ts.symbol not in active_kal:
                stale.append(ts)
        for ts in stale:
            self._untrack(ts)

    def _track(
        self,
        exchange: str,
        symbol: str,
        label: str,
        rotates: bool,
        window_end: Optional[int] = None,
    ) -> None:
        key = f"{exchange}:{symbol}"
        if key in self._tracked:
            return
        self._tracked[key] = TrackedSymbol(
            exchange=exchange,
            symbol=symbol,
            label=label,
            rotates=rotates,
            window_end=window_end,
        )
        self.feed.subscribe([symbol], exchange=exchange)
        # Also follow the public last-trade stream for this symbol so the
        # status table can show executed prints next to the BBA.
        self.feed.subscribe_trades([symbol], exchange=exchange)
        log.info("subscribe %s (%s)", label, key)

    def _untrack(self, ts: TrackedSymbol) -> None:
        key = f"{ts.exchange}:{ts.symbol}"
        log.info("unsubscribe %s (window rotated)", ts.label)
        self.feed.unsubscribe([ts.symbol], exchange=ts.exchange)
        self.feed.unsubscribe_trades([ts.symbol], exchange=ts.exchange)
        self._tracked.pop(key, None)

    # ── reporting ──

    def _report(self) -> None:
        if not self._tracked:
            log.info("(no subscriptions yet)")
            return

        now_ns = time.time_ns()
        rows: list[tuple[str, str]] = []  # (group key, formatted row)
        for ts in self._tracked.values():
            quote = self.feed.latest(ts.symbol, exchange=ts.exchange)
            trade = self.feed.latest_trade(ts.symbol, exchange=ts.exchange)
            rows.append((ts.exchange, _format_row(ts, quote, trade, now_ns)))

        # Group by exchange for readability.
        rows.sort(key=lambda r: (r[0], r[1]))
        log.info("─── snapshot ─────────────────────────────────────────────")
        last_group = None
        for group, row in rows:
            if group != last_group:
                log.info("[%s]", group)
                last_group = group
            log.info("  %s", row)


# ── Helpers ─────────────────────────────────────────────────────────────────


def _format_trade(trade: Optional[Trade], now_ns: int) -> str:
    if trade is None:
        return "last=          —"
    t_age_ms = (now_ns - trade.received_ns) / 1e6
    px = f"{trade.price:.6f}"
    return f"last={px:>10s}x{trade.size:<8.4g}{trade.side:<4s} ({t_age_ms:6.0f}ms)"


def _format_row(
    ts: TrackedSymbol,
    quote: Optional[Quote],
    trade: Optional[Trade],
    now_ns: int,
) -> str:
    tr = _format_trade(trade, now_ns)
    if quote is None:
        return f"{ts.label:48s}  (no quote yet)  {tr}"
    age_ms = (now_ns - quote.received_ns) / 1e6
    bid = f"{quote.best_bid:.6f}" if quote.best_bid is not None else "—"
    ask = f"{quote.best_ask:.6f}" if quote.best_ask is not None else "—"
    mid = f"{quote.mid:.6f}" if quote.mid is not None else "—"
    age = f"{age_ms:7.0f}ms"
    win = ""
    if ts.window_end is not None:
        remaining = ts.window_end - int(time.time())
        win = f"  win-end-in={remaining:>4d}s"
    return (
        f"{ts.label:48s}  bid={bid:>10s}  ask={ask:>10s}  mid={mid:>10s}  "
        f"age={age}  {tr}{win}"
    )
