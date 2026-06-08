"""Probe strategy — place + cancel at the price floor on every quote.

Event-driven. Subscribes one Polymarket quote callback; on each fire,
runs the pin/safe-mid guard, places at MIN_PX, then cancels in the
same callback (synchronous REST round-trips).

Safety properties:

  1. Price = MIN_PX. Every standing bid is strictly higher, so the
     matching engine cannot match the order against any resting sell.
     No partial fills possible.
  2. Place and cancel happen back-to-back inside one callback —
     no other event can run between them on the dispatcher thread.
"""

from __future__ import annotations

import logging
import time

from ...market import Quote
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import MIN_PX, PIN_PX, SAFE_MID_HIGH, SAFE_MID_LOW, safe_mid_guard

log = logging.getLogger(__name__)


class ProbeStrategy(Strategy):
    name = "probe"
    is_window = True  # built with a PolymarketWindow by Strategy.build

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
            raise ValueError("ProbeStrategy needs a Polymarket window")
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high
        # Live oid stash so a failed cancel can retry next quote.
        self._stale_oid: str | None = None

    def required_topics(self) -> list[tuple[str, str, str]]:
        # Rolling Polymarket topic. Snapshot only — trades aren't needed.
        return [("polymarket", self.window.base_slug, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        # Probe is meant to exercise the order pipe — fire on every
        # quote, overriding the dispatcher's default 2s throttle.
        dispatcher.register_quote(
            "polymarket", self.window.base_slug, self._on_quote, min_interval_ms=0
        )
        dispatcher.register_rollover(
            "polymarket", self.window.base_slug, self._on_rollover
        )
        dispatcher.register_bootstrap(
            "polymarket", self.window.base_slug, self._on_bootstrap
        )
        dispatcher.register_order_update(self._on_order_update)

    # ── event handlers ──

    def _on_bootstrap(self, full_slug: str) -> None:
        # No-op stub. MarketState.on_bootstrap already populated
        # asset_ids; the engine pre-stamped self.window.full_slug.
        pass

    def _on_rollover(self, full_slug: str) -> None:
        # asset_ids already resolved by MarketState.on_rollover.
        old = self.window.full_slug
        self.window.full_slug = full_slug
        if old is None:
            log.info(
                f"[{self.window.base_slug}] probe bootstrap: full_slug={full_slug}"
            )
        else:
            log.info(f"[{self.window.base_slug}] probe rollover: {old} → {full_slug}")

    def _on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None:
            return
        if status in {"filled", "canceled", "rejected", "expired"}:
            self.state.orders.mark(oid, status)
            if oid == self._stale_oid:
                self._stale_oid = None

    def _on_quote(self, q: Quote) -> None:
        asset_id = self._asset_id("up")
        if asset_id is None:
            log.debug(f"[{self.label}] probe bootstrapping (no UP asset_id), skipping")
            return

        reason = safe_mid_guard(
            self.market,
            "polymarket",
            self.window.base_slug,
            low=self.safe_mid_low,
            high=self.safe_mid_high,
            pin_px=self.pin_px,
        )
        if reason is not None:
            return

        # If a prior cancel failed, retry it before placing a fresh one.
        if self._stale_oid is not None:
            try:
                self.router.cancel(self._stale_oid, **self._attribution())
                log.info(f"[{self.label}] PROBE stale cancel ok oid={self._stale_oid}")
                self._stale_oid = None
            except Exception as e:  # noqa: BLE001
                log.warning(f"[{self.label}] PROBE stale cancel still failing: {e}")
                return

        target_px = MIN_PX
        target_qty = max(round(1.0 / target_px, 2), 1.0)
        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-probe-{slug[:18]}-{int(time.time() * 1000)}"

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
            log.warning(f"[{self.label}] PROBE place failed: {e}")
            return

        oid = ack.get("exchange_oid")
        log.info(
            f"[{self.label}] PROBE place px={target_px:.2f} "
            f"qty={target_qty:.2f} oid={oid}"
        )

        if oid is None:
            return
        try:
            self.router.cancel(oid, **attribution)
            log.info(f"[{self.label}] PROBE cancel ok oid={oid}")
        except Exception as e:  # noqa: BLE001
            log.warning(
                f"[{self.label}] PROBE cancel failed (will retry next quote): {e}"
            )
            self._stale_oid = oid

    def teardown(self) -> None:
        if self._stale_oid is not None:
            try:
                self.router.cancel(self._stale_oid, **self._attribution())
            except Exception as e:  # noqa: BLE001
                log.warning(f"[{self.label}] probe teardown cancel failed: {e}")
            self._stale_oid = None
