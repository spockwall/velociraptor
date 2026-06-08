"""Kalshi fill-once — one market order on the first valid quote, then park.

The Kalshi analogue of `fill_once`. Kalshi is a non-window exchange in the
engine: market data arrives on the stable `series` topic (e.g. `KXBTC15M`), the
same way Polymarket arrives on `base_slug`. Orders, however, target a specific
**market ticker** (e.g. `KXBTC15M-26JUN081145-15`), supplied explicitly via
`--kalshi-ticker` (Kalshi market tickers carry a strike/close suffix the quote
`series` does not, and the venue 404s `market_not_found` on a bare series).

On the first quote that shows a usable book, fires one IOC/FOK **market** order
via `OrderRouter.place_market` (the executor's Kalshi client now supports
`type:"market"`) and latches `_sent`. Market orders fill or fail immediately, so
there's no resting state to track and nothing to cancel.

Unlike the Polymarket window strategies, there is no rollover/asset-id dance —
the ticker is fixed for the run. Restart the engine (with a new `--kalshi-ticker`)
to trade the next window.
"""

from __future__ import annotations

import logging
import time

from ...market import Quote
from ..dispatcher import Dispatcher
from .base import Strategy

log = logging.getLogger(__name__)


class KalshiFillOnceStrategy(Strategy):
    name = "kalshi_fill_once"
    # Orders route to Kalshi, not the default Polymarket.
    order_exchange = "kalshi"

    @classmethod
    def build(cls, args, *, market, router, state):
        series = args.kalshi_series
        if not series:
            raise SystemExit("--strategy kalshi_fill_once needs --kalshi-series")
        if not args.kalshi_ticker:
            raise SystemExit(
                "--strategy kalshi_fill_once needs --kalshi-ticker "
                "(UPPERCASE market ticker, e.g. KXBTC15M-26JUN081145-15)"
            )
        return cls(
            market=market,
            router=router,
            state=state,
            kalshi_series=series[0],
            kalshi_ticker=args.kalshi_ticker,
            side=args.kalshi_side,
            order_qty=args.kalshi_qty,
            market_tif=args.kalshi_tif,
        )

    def __init__(
        self,
        *,
        kalshi_series: str,
        kalshi_ticker: str,
        side: str = "buy",
        order_qty: float = 1.0,
        market_tif: str = "IOC",
        **kwargs,
    ):
        super().__init__(**kwargs)
        if not kalshi_series:
            raise ValueError("KalshiFillOnceStrategy needs --kalshi-series")
        if not kalshi_ticker:
            raise ValueError(
                "KalshiFillOnceStrategy needs --kalshi-ticker (UPPERCASE market "
                "ticker, e.g. KXBTC15M-26JUN081145-15)"
            )
        self.series = kalshi_series
        self.ticker = kalshi_ticker
        self.side = side
        self.order_qty = order_qty
        self.market_tif = market_tif.upper()
        self._sent = False

    # ── data dependencies ──

    def required_topics(self) -> list[tuple[str, str, str]]:
        # Quotes arrive on the stable Kalshi series topic.
        return [("kalshi", self.series, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        # min_interval_ms=0: fire on the first valid book; the `_sent` latch
        # makes every subsequent quote a no-op.
        dispatcher.register_quote(
            "kalshi", self.series, self._on_quote, min_interval_ms=0
        )
        dispatcher.register_order_update(self._on_order_update)
        dispatcher.register_fill(self._on_fill)

    # ── handlers ──

    def _on_quote(self, q: Quote) -> None:
        if self._sent:
            return
        # Freshness gate: only fire once we've seen a usable book. A market
        # order doesn't need the price (the venue matches against resting
        # liquidity), but we want proof the feed is alive before trading.
        if q.best_bid is None and q.best_ask is None and q.mid is None:
            log.debug(f"[{self.label}] skip: no book in latest quote yet")
            return

        qty = round(self.order_qty, 2)
        if qty <= 0:
            log.debug(f"[{self.label}] skip: order_qty <= 0")
            return

        # Deterministic per (ticker, side) so a repeat fire (retry / replay)
        # re-uses the same id and hits the executor's idempotency cache rather
        # than placing a second order. Belt-and-braces on the `_sent` latch.
        client_oid = f"te-kalshi-mkt-{self.ticker[:24]}-{self.side[:1]}"
        # Latch BEFORE the call: one run = one attempt, success or failure.
        self._sent = True
        log.info(
            f"[{self.label}] KALSHI_FILL_ONCE place_market ticker={self.ticker} "
            f"side={self.side} qty={qty} tif={self.market_tif} client_oid={client_oid}"
        )
        try:
            # place_market logs the ack centrally; nothing extra to log here.
            self.router.place_market(
                symbol=self.ticker,
                side=self.side,
                qty=qty,
                client_oid=client_oid,
                tif=self.market_tif,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            # Latch stays True — one attempt per run. Inspect logs / audit.
            log.warning(f"[{self.label}] KALSHI_FILL_ONCE place_market failed: {e}")

    def _on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None:
            return
        if status in {"filled", "canceled", "rejected", "expired"}:
            self.state.orders.mark(oid, status)

    def _on_fill(self, ev: dict) -> None:
        # Only count fills on our ticker.
        if ev.get("symbol") != self.ticker:
            return
        log.info(
            f"[{self.label}] KALSHI_FILL_ONCE filled "
            f"px={ev.get('px')} qty={ev.get('qty')} "
            f"oid={ev.get('exchange_oid')} trade={ev.get('trade_id')}"
        )

    @property
    def label(self) -> str:
        # Non-window strategy: identify by the traded ticker.
        return self.ticker or self.name

    # ── teardown ──

    def teardown(self) -> None:
        # Market orders are terminal on the venue (IOC/FOK) — nothing to cancel.
        return
