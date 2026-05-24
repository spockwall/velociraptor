"""Fill-once strategy — cross the spread, fill exactly once per window.

Single Polymarket quote callback. Quotes ask + N ticks (crosses) so the
order takes immediately. After a fill arrives, parks until the next
window — `_on_rollover` clears the `filled` latch.
"""

from __future__ import annotations

import logging
import time
from typing import Optional

from ...market import Quote
from ...typings.state import OrderRecord
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import (
    PIN_PX,
    SAFE_MID_HIGH,
    SAFE_MID_LOW,
    TICK,
    clamp_px,
    qty_for_notional,
    safe_mid_guard,
)

log = logging.getLogger(__name__)


class FillOnceStrategy(Strategy):
    name = "fill_once"

    def __init__(
        self,
        *,
        tight_offset_ticks: int = 1,
        order_notional_usd: float = 10.0,
        pin_px: float = PIN_PX,
        safe_mid_low: float = SAFE_MID_LOW,
        safe_mid_high: float = SAFE_MID_HIGH,
        **kwargs,
    ):
        super().__init__(**kwargs)
        if self.window is None:
            raise ValueError("FillOnceStrategy needs a Polymarket window")
        self.tight_offset_ticks = tight_offset_ticks
        self.order_notional_usd = order_notional_usd
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high
        # Window-scoped state.
        self._live_oid: Optional[str] = None
        self._live_px: Optional[float] = None
        self._live_qty: Optional[float] = None
        self._filled_this_window: bool = False

    def required_topics(self) -> list[tuple[str, str, str]]:
        return [("polymarket", self.window.base_slug, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        # React to every quote so the resting order tracks the touch
        # without 2s lag (overrides dispatcher's default throttle).
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
        # New window → clear all window-scoped state.
        self._live_oid = None
        self._live_px = None
        self._live_qty = None
        self._filled_this_window = False

    def _on_quote(self, q: Quote) -> None:
        if self._filled_this_window:
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
            if self._live_oid is not None:
                self._cancel_live()
            return

        if q.best_ask is None:
            return
        target_px = clamp_px(q.best_ask + self.tight_offset_ticks * TICK)
        target_qty = qty_for_notional(self.order_notional_usd, target_px)
        if target_qty <= 0:
            return

        # Idempotent — same target as current resting order → no-op.
        if (
            self._live_oid is not None
            and self._live_px == target_px
            and self._live_qty == target_qty
        ):
            return

        if self._live_oid is not None:
            self._cancel_live()

        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-fill-{slug[:18]}-{int(time.time() * 1000)}"
        try:
            ack = self.router.place_limit(
                symbol=asset_id,
                side="buy",
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] FILL_ONCE place failed: {e}")
            return

        oid = ack.get("exchange_oid")
        self._live_oid = oid
        self._live_px = target_px
        self._live_qty = target_qty
        # Record into the ledger so audit + window queries stay consistent.
        self.state.orders.add(
            OrderRecord(
                client_oid=client_oid,
                side_name="up",
                symbol=asset_id,
                direction="buy",
                px=target_px,
                qty=target_qty,
                exchange_oid=oid,
            )
        )
        log.info(
            f"[{self.label}] FILL_ONCE place px={target_px:.2f} "
            f"qty={target_qty:.2f} oid={oid}"
        )

    def _on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None:
            return
        if status in {"filled", "canceled", "rejected", "expired"}:
            self.state.orders.mark(oid, status)
            if oid == self._live_oid:
                self._live_oid = None
                self._live_px = None
                self._live_qty = None

    def _on_fill(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        if oid is None:
            return
        if oid == self._live_oid or (
            self.window.up_asset_id is not None
            and self.window.up_asset_id == ev.get("symbol")
        ):
            self._filled_this_window = True
            log.info(
                f"[{self.label}] FILL_ONCE filled "
                f"px={ev.get('px')} qty={ev.get('qty')} oid={oid}"
            )

    # ── teardown ──

    def _cancel_live(self) -> None:
        if self._live_oid is None:
            return
        try:
            self.router.cancel(self._live_oid, **self._attribution())
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] FILL_ONCE cancel failed: {e}")
        self._live_oid = None
        self._live_px = None
        self._live_qty = None

    def teardown(self) -> None:
        self._cancel_live()
