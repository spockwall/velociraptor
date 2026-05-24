"""One-shot strategy — place once at the floor, cancel once, stop.

Single Polymarket quote callback. On first valid quote (passes the
pin / safe-mid guard), place at MIN_PX, cancel immediately, then call
`dispatcher.stop()` so the engine exits cleanly.

Two safety properties (same as `probe`):

  1. Price = MIN_PX → no fill possible (every standing bid is higher).
  2. Place + cancel run back-to-back inside one callback; no other
     event can interleave.
"""

from __future__ import annotations

import logging
import time

from ...market import Quote
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import MIN_PX, PIN_PX, SAFE_MID_HIGH, SAFE_MID_LOW, safe_mid_guard

log = logging.getLogger(__name__)


class OneShotStrategy(Strategy):
    name = "one_shot"

    def __init__(
        self,
        *,
        pin_px: float = PIN_PX,
        safe_mid_low: float = SAFE_MID_LOW,
        safe_mid_high: float = SAFE_MID_HIGH,
        **kwargs,
    ):
        super().__init__(**kwargs)
        if self.window is None:
            raise ValueError("OneShotStrategy needs a Polymarket window")
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high
        self._done = False
        self._dispatcher: Dispatcher | None = None
        self._stale_oid: str | None = None

    def required_topics(self) -> list[tuple[str, str, str]]:
        return [("polymarket", self.window.base_slug, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        self._dispatcher = dispatcher
        # Fire on the first valid quote (no 2s default-throttle delay).
        dispatcher.register_quote(
            "polymarket", self.window.base_slug, self._on_quote, min_interval_ms=0
        )
        dispatcher.register_rollover(
            "polymarket", self.window.base_slug, self._on_rollover
        )

    def _on_rollover(self, full_slug: str, asset_id: str) -> None:
        self.window.full_slug = full_slug
        self.window.up_asset_id = asset_id

    def _on_quote(self, q: Quote) -> None:
        if self._done:
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

        target_px = MIN_PX
        target_qty = max(round(1.0 / target_px, 2), 1.0)
        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-1shot-{slug[:18]}-{int(time.time() * 1000)}"

        attribution = self._attribution()
        try:
            ack = self.router.place_limit(
                symbol=asset_id,
                side="buy",
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
                **attribution,
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] ONE_SHOT place failed: {e}")
            return

        oid = ack.get("exchange_oid")
        log.info(
            f"[{self.label}] ONE_SHOT place px={target_px:.2f} "
            f"qty={target_qty:.2f} oid={oid}"
        )
        if oid is None:
            self._finish("no exchange_oid returned")
            return

        try:
            self.router.cancel(oid, **attribution)
            log.info(f"[{self.label}] ONE_SHOT cancel ok oid={oid}")
            self._finish("place+cancel ok")
        except Exception as e:  # noqa: BLE001
            log.warning(
                f"[{self.label}] ONE_SHOT cancel failed; keeping oid for teardown: {e}"
            )
            self._stale_oid = oid
            # Don't finish — let teardown retry the cancel on exit.

    def _finish(self, why: str) -> None:
        self._done = True
        log.info(f"[{self.label}] ONE_SHOT done ({why}) — stopping dispatcher")
        if self._dispatcher is not None:
            self._dispatcher.stop()

    def teardown(self) -> None:
        if self._stale_oid is not None:
            try:
                self.router.cancel(self._stale_oid, **self._attribution())
            except Exception as e:  # noqa: BLE001
                log.warning(f"[{self.label}] one_shot teardown cancel failed: {e}")
            self._stale_oid = None
