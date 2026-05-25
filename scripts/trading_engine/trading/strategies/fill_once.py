"""Fill-once strategy — one market order per window, then park.

Single Polymarket quote callback. On the first quote that passes the
safe-mid guard, fires an IOC market order via `OrderRouter.place_market`
and latches `_filled_this_window`. A subsequent rollover clears the
latch for the new window.

Market orders fill or fail immediately, so there's no resting state
to track — no cancel/replace dance. The `q` argument to `_on_quote`
is still used as a freshness signal (we only place after seeing at
least one valid book), but the price comes from the venue's match
engine, not from us.
"""

from __future__ import annotations

import logging
import time

from ...market import Quote
from ...typings.state import OrderRecord
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import (
    PIN_PX,
    SAFE_MID_HIGH,
    SAFE_MID_LOW,
    qty_for_notional,
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
        self.window.full_slug = full_slug
        self.window.up_asset_id = asset_id
        # New window → clear latches so we fire again.
        self._sent_this_window = False
        self._filled_this_window = False

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

        # Use the ask as the px reference *only* for qty sizing
        # (notional / px → shares). The exchange ignores px on a
        # market order; we just need a non-zero reference.
        ref_px = q.best_ask if q.best_ask is not None else q.mid
        if ref_px is None or ref_px <= 0:
            return
        target_qty = qty_for_notional(self.order_notional_usd, ref_px)
        if target_qty <= 0:
            return

        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-fill-mkt-{slug[:18]}-{int(time.time() * 1000)}"
        try:
            ack = self.router.place_market(
                symbol=asset_id,
                side="buy",
                qty=target_qty,
                client_oid=client_oid,
                tif=self.market_tif,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] FILL_ONCE place_market failed: {e}")
            return

        # Latch immediately so a subsequent quote in the same window
        # doesn't re-fire even if the ack races the next callback.
        self._sent_this_window = True
        oid = ack.get("exchange_oid")
        self.state.orders.add(
            OrderRecord(
                client_oid=client_oid,
                side_name="up",
                symbol=asset_id,
                direction="buy",
                px=ref_px,  # reference only — market order has no limit
                qty=target_qty,
                exchange_oid=oid,
            )
        )
        log.info(
            f"[{self.label}] FILL_ONCE place_market qty={target_qty:.2f} "
            f"tif={self.market_tif} ref_ask={ref_px:.4f} oid={oid}"
        )

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
