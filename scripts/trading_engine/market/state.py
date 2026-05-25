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

import logging
from collections import deque
from typing import Deque, Optional

from .feed import Quote, Snapshot, Trade
from .gamma import fetch_asset_ids

log = logging.getLogger(__name__)

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
        # Current full_slug per (exchange, base_slug) for rolling topics.
        # Updated by `_update_rollover` on every RolloverEvent.
        self._current_full_slug: dict[tuple[str, str], str] = {}
        # Resolved (up_asset_id, down_asset_id) per full_slug. Populated
        # by strategies (or by an engine-level helper) via Gamma on
        # rollover; `None` entries are allowed if resolution failed and
        # strategies should NOT trade until present.
        self._asset_ids: dict[str, tuple[Optional[str], Optional[str]]] = {}

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
        """Copy of every Quote currently held. Used by observe-style
        callbacks that print a unified table."""
        return dict(self._quotes)

    def trades_all(self) -> dict[tuple[str, str], Trade]:
        return dict(self._trades)

    def current_full_slug(self, exchange: str, base_slug: str) -> Optional[str]:
        """For rolling topics: the latest `full_slug` seen on
        `polymarket:{base_slug}` / `kalshi:{series}`. `None` until the
        first frame arrives."""
        return self._current_full_slug.get((exchange, base_slug))

    def asset_ids(
        self, full_slug: str
    ) -> Optional[tuple[Optional[str], Optional[str]]]:
        """Resolved `(up_asset_id, down_asset_id)` for a Polymarket
        rolling window. `None` if the engine hasn't resolved this
        full_slug yet — strategies MUST guard on this and refuse to
        trade when it returns `None`."""
        return self._asset_ids.get(full_slug)

    def set_asset_ids(
        self,
        full_slug: str,
        up_asset_id: Optional[str],
        down_asset_id: Optional[str],
    ) -> None:
        """Populate the (up, down) entry for a Polymarket full_slug.
        Normally called by `on_rollover`; exposed for tests and any
        external resolver (e.g. backend pre-seed)."""
        self._asset_ids[full_slug] = (up_asset_id, down_asset_id)

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

    def _update_current_full_slug(
        self, exchange: str, base_slug: str, full_slug: str
    ) -> None:
        """Internal write helper. Advances the last-seen `full_slug`
        without touching asset_ids — used by both the bootstrap and
        rollover code paths."""
        self._current_full_slug[(exchange, base_slug)] = full_slug

    def _refresh_window(
        self, exchange: str, base_slug: str, full_slug: str, log_prefix: str
    ) -> None:
        """Shared work for `on_bootstrap` and `on_rollover`:
        advance `_current_full_slug`, then for Polymarket resolve
        UP/DOWN asset_ids via Gamma and stamp `_asset_ids[full_slug]`.

        `log_prefix` is `"bootstrap"` or `"rollover"` so the operator
        can tell which trigger ran in the logs. Kalshi exchanges skip
        the Gamma call (the ticker IS the asset id; not a Gamma slug).
        Failures are logged and stored as `(None, None)` so strategies
        that guard on `asset_ids()` correctly refuse to trade.
        """
        self._update_current_full_slug(exchange, base_slug, full_slug)
        if exchange != "polymarket":
            return
        ids = fetch_asset_ids(full_slug)
        if ids is None:
            log.warning(
                "%s: gamma resolve failed for %s; trading disabled "
                "until next rollover",
                log_prefix,
                full_slug,
            )
            self._asset_ids[full_slug] = (None, None)
            return
        up, down = ids
        self._asset_ids[full_slug] = (up, down)
        log.info(
            "%s: resolved asset_ids for %s: up=%s… down=%s…",
            log_prefix,
            full_slug,
            up[:12] if up else None,
            down[:12] if down else None,
        )

    def on_bootstrap(
        self, exchange: str, base_slug: str, full_slug: str
    ) -> None:
        """Called by the dispatcher on `BootstrapEvent` (engine
        startup, once per window-strategy)."""
        self._refresh_window(exchange, base_slug, full_slug, "bootstrap")

    def on_rollover(
        self, exchange: str, base_slug: str, full_slug: str
    ) -> None:
        """Called by the dispatcher on `RolloverEvent` (wire-detected
        `full_slug` change)."""
        self._refresh_window(exchange, base_slug, full_slug, "rollover")
