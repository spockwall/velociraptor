"""Live cross-source market state.

`MarketState` is the engine's single picture of the world. The
`Dispatcher` updates it as raw `QuoteEvent` / `TradeEvent`s pop off
the queue; the `Strategy`'s registered callbacks then read it through
the convenience methods below.

Why this exists as its own class (instead of leaving the cache inside
`MarketFeed`):

  - `MarketFeed` is the ZMQ transport. Strategies should not depend
    on a transport object; they should depend on the *data*.
  - The transport runs on its own thread (ZMQ recv loop); the
    dispatcher runs on a separate thread. Decoupling state ownership
    from the producer thread makes the locking model trivial: the
    dispatcher is the sole writer, and the strategy's callbacks run
    on the dispatcher thread, so reads are uncontended without any
    lock.
  - Strategies frequently need derived views (`mid`, recent trades,
    "the latest of everything"). Putting the helpers on the data
    object keeps callers terse and avoids per-strategy reimplementation.

Threading model: single-writer (Dispatcher thread) / single-reader
(strategy callbacks, which the Dispatcher invokes on the same thread).
No locks are taken. If a future producer ever needs to call
`_update_*` from a different thread, add a lock here and adjust the
docstring — but the design intent is to push events through the
dispatcher instead.
"""

from __future__ import annotations

from collections import deque
from typing import Deque, Optional

from .feed import Quote, Snapshot, Trade

_DEFAULT_TRADE_HISTORY = 64


class MarketState:
    """Cross-source live market data. One instance per `Engine`."""

    def __init__(self, trade_history: int = _DEFAULT_TRADE_HISTORY) -> None:
        self._trade_history = trade_history
        # Latest BBA quote per (exchange, symbol).
        self._quotes: dict[tuple[str, str], Quote] = {}
        # Latest full snapshot (with depth) per (exchange, symbol).
        self._snapshots: dict[tuple[str, str], Snapshot] = {}
        # Latest single trade (mirrors old `latest_trade`).
        self._trades: dict[tuple[str, str], Trade] = {}
        # Bounded trade history per (exchange, symbol). Strategies that
        # need a window of prints (e.g. momentum) read this.
        self._trade_hist: dict[tuple[str, str], Deque[Trade]] = {}
        # Last seen full_slug per (exchange, base_slug) for rolling
        # topics. Mirrors `MarketFeed._last_full_slug` so callers that
        # want the current asset_id without parsing payloads can read
        # `current_asset_id(...)` straight off the state.
        self._current_asset_id: dict[tuple[str, str], str] = {}

    # ── reads (strategy-facing) ──

    def quote(self, exchange: str, symbol: str) -> Optional[Quote]:
        return self._quotes.get((exchange, symbol))

    def snapshot(self, exchange: str, symbol: str) -> Optional[Snapshot]:
        """Full-depth orderbook snapshot, if a `SnapshotEvent` has
        landed for this (exchange, symbol). `None` until then."""
        return self._snapshots.get((exchange, symbol))

    def trade(self, exchange: str, symbol: str) -> Optional[Trade]:
        return self._trades.get((exchange, symbol))

    def mid(self, exchange: str, symbol: str) -> Optional[float]:
        q = self._quotes.get((exchange, symbol))
        if q is None:
            return None
        if q.mid is not None:
            return q.mid
        if q.best_bid is not None and q.best_ask is not None:
            return (q.best_bid + q.best_ask) / 2.0
        return None

    def best_bid(self, exchange: str, symbol: str) -> Optional[float]:
        q = self._quotes.get((exchange, symbol))
        return q.best_bid if q is not None else None

    def best_ask(self, exchange: str, symbol: str) -> Optional[float]:
        q = self._quotes.get((exchange, symbol))
        return q.best_ask if q is not None else None

    def recent_trades(self, exchange: str, symbol: str, n: int) -> list[Trade]:
        """The last `n` trades for (exchange, symbol), newest last.
        Returns at most `n`; fewer when not enough history has accumulated."""
        hist = self._trade_hist.get((exchange, symbol))
        if hist is None:
            return []
        if n >= len(hist):
            return list(hist)
        # deque slicing is not supported; build from the tail.
        return list(hist)[-n:]

    def snapshot_all(self) -> dict[tuple[str, str], Quote]:
        """Copy of every Quote currently held. Used by observer-style
        callbacks that print a unified table."""
        return dict(self._quotes)

    def trades_all(self) -> dict[tuple[str, str], Trade]:
        return dict(self._trades)

    def current_asset_id(self, exchange: str, base_slug: str) -> Optional[str]:
        """For rolling topics: the asset_id (= full_slug-side) currently
        backing this base_slug. None until the first frame arrives."""
        return self._current_asset_id.get((exchange, base_slug))

    # ── writes (dispatcher-only) ──

    def _update_quote(self, exchange: str, symbol: str, q: Quote) -> None:
        self._quotes[(exchange, symbol)] = q

    def _update_snapshot(self, exchange: str, symbol: str, s: Snapshot) -> None:
        self._snapshots[(exchange, symbol)] = s

    def _update_trade(self, exchange: str, symbol: str, t: Trade) -> None:
        self._trades[(exchange, symbol)] = t
        hist = self._trade_hist.get((exchange, symbol))
        if hist is None:
            hist = deque(maxlen=self._trade_history)
            self._trade_hist[(exchange, symbol)] = hist
        hist.append(t)

    def _update_rollover(self, exchange: str, base_slug: str, asset_id: str) -> None:
        self._current_asset_id[(exchange, base_slug)] = asset_id
