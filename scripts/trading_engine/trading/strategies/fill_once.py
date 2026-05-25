"""Fill-once strategy — one market order per window, then park.

Single Polymarket quote callback. On the first quote that passes the
safe-mid guard, fires an IOC market order via `OrderRouter.place_market`
and latches `_sent_this_window`. A rollover clears the latch for the
new window.

Market orders fill or fail immediately, so there's no resting state
to track — no cancel/replace dance. The `q` argument to `_on_quote`
is just a freshness signal (we only place after seeing at least one
valid book); the price comes from the venue's match engine.
"""

from __future__ import annotations

import logging

from ...market import Quote
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import (
    PIN_PX,
    SAFE_MID_HIGH,
    SAFE_MID_LOW,
    safe_mid_guard,
)

log = logging.getLogger(__name__)


class FillOnceStrategy(Strategy):
    name = "fill_once"

    def __init__(
        self,
        *,
        order_notional_usd: float = 10.0,
        pin_px: float = PIN_PX,
        safe_mid_low: float = SAFE_MID_LOW,
        safe_mid_high: float = SAFE_MID_HIGH,
        market_tif: str = "IOC",
        **kwargs,
    ):
        super().__init__(**kwargs)
        if self.window is None:
            raise ValueError("FillOnceStrategy needs a Polymarket window")
        self.order_notional_usd = order_notional_usd
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high
        self.market_tif = market_tif.upper()
        # Window-scoped state.
        self._sent_this_window: bool = False
        self._filled_this_window: bool = False

    def required_topics(self) -> list[tuple[str, str, str]]:
        return [("polymarket", self.window.base_slug, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        # Quote callback as a freshness gate — we want at least one
        # valid book before firing. Min-interval=0 because we want to
        # fire as soon as the safe-mid guard passes; everything after
        # the first send is a no-op via the `_sent_this_window` latch.
        dispatcher.register_quote(
            "polymarket",
            self.window.base_slug,
            self._on_quote,
            min_interval_ms=0,
        )
        dispatcher.register_rollover(
            "polymarket", self.window.base_slug, self._on_rollover
        )
        dispatcher.register_order_update(self._on_order_update)
        dispatcher.register_fill(self._on_fill)

    # ── handlers ──

    def _on_rollover(self, full_slug: str, asset_id: str) -> None:
        # The zmq_server scheduler now spawns each window only AT the
        # boundary (no pre-spawn), so a rollover event corresponds 1:1
        # to a real window transition — no flap, no re-emit.
        prev_full = self.window.full_slug
        self.window.full_slug = full_slug
        self.window.up_asset_id = asset_id
        self._sent_this_window = False
        self._filled_this_window = False
        log.info(
            f"[{self.label}] rollover {prev_full!r} → {full_slug!r} "
            f"asset={asset_id}"
        )

    def _on_quote(self, q: Quote) -> None:
        if self._sent_this_window:
            return
        asset_id = self.window.up_asset_id
        if asset_id is None:
            return

        if (
            safe_mid_guard(
                self.market,
                "polymarket",
                self.window.base_slug,
                low=self.safe_mid_low,
                high=self.safe_mid_high,
                pin_px=self.pin_px,
            )
            is not None
        ):
            return

        # Freshness gate: only fire after seeing at least one valid
        # book. The price itself isn't needed — for a Polymarket market
        # BUY we send USDC notional, and the venue walks the asks
        # itself.
        if q.best_ask is None and q.mid is None:
            return

        # Polymarket market BUY: `qty` is USDC notional, rounded to 2
        # decimals so the venue's precision check on the maker amount
        # passes (it rejects >2 decimal places).
        target_qty_usdc = round(self.order_notional_usd, 2)
        if target_qty_usdc <= 0:
            return

        slug = self.window.full_slug or self.window.base_slug
        # `client_oid` is deterministic per (window, asset_id) so that a
        # repeat fire — for any reason: rollover re-emit, snapshot
        # replay, retry storm — re-uses the same id and hits the
        # executor's idempotency cache instead of placing a new order.
        # This is a belt-and-braces fallback on top of the
        # `_sent_this_window` latch + the `_on_rollover` no-change guard.
        client_oid = f"te-fill-mkt-{slug[:24]}-{asset_id[:10]}"
        # Latch BEFORE the call: one window = one attempt, regardless of
        # success or failure. Any response (ack or exception) is terminal
        # for the window; the next firing waits for rollover.
        self._sent_this_window = True
        log.info(
            f"[{self.label}] FILL_ONCE place_market notional=${target_qty_usdc:.2f} "
            f"tif={self.market_tif} client_oid={client_oid}"
        )
        try:
            ack = self.router.place_market(
                symbol=asset_id,
                side="buy",
                qty=target_qty_usdc,
                client_oid=client_oid,
                tif=self.market_tif,
                **self._attribution(),
            )
            log.info(
                f"[{self.label}] FILL_ONCE ack "
                f"oid={ack.get('exchange_oid')} status={ack.get('status')}"
            )
        except Exception as e:  # noqa: BLE001
            # Latch stays True: one window = one attempt. Investigate
            # in the logs / executor audit; we will not retry until
            # the next rollover.
            log.warning(f"[{self.label}] FILL_ONCE place_market failed: {e}")

    def _on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None:
            return
        if status in {"filled", "canceled", "rejected", "expired"}:
            self.state.orders.mark(oid, status)

    def _on_fill(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        if oid is None:
            return
        if (
            self.window.up_asset_id is not None
            and self.window.up_asset_id == ev.get("symbol")
        ):
            self._filled_this_window = True
            log.info(
                f"[{self.label}] FILL_ONCE filled "
                f"px={ev.get('px')} qty={ev.get('qty')} oid={oid}"
            )

    # ── teardown ──

    def teardown(self) -> None:
        # Market orders are fire-and-forget (IOC/FOK both terminal on
        # the venue side). Nothing to cancel here.
        return
