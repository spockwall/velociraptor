"""Probe strategy — no-fill correctness check.

Place + cancel back-to-back on both YES and NO. Two safety properties:

1. **Price is the floor** (`MIN_PX = 0.01`). At $0.01 buy, the order is
   below every standing bid on the book, so the matching engine cannot
   match it against any resting sell. No partial fills possible.

2. **Cancel is synchronous and immediate.** After `place_limit` returns
   the exchange_oid, we issue `cancel(...)` in the same tick — within
   the same Python coroutine, sub-second turnaround bounded only by
   the two REST round-trips. The order's lifetime on the book is the
   network latency between the two calls.

The tick loop still calls us periodically; each tick is a fresh
place-then-cancel pair. The `--tick-secs` flag controls how often
we run another probe.

Used to be `--step 1`.
"""

from __future__ import annotations

import logging
import time
from typing import Optional

from ...market import Quote
from .base import MIN_PX, SideState, Strategy

log = logging.getLogger(__name__)


class ProbeStrategy(Strategy):
    name = "probe"

    def _target_px(self, quote: Quote) -> Optional[float]:
        # Always quote at the absolute floor. No fill is possible at this
        # price because every standing bid is strictly higher.
        return MIN_PX

    def _tick_one_side(self, side_name: str, side: SideState) -> None:
        """Override the base tick so place + cancel happen back-to-back
        with no intervening wait. We don't reuse the parent's
        place-and-leave-resting flow because that exposes the order to
        the book until the next tick (potentially seconds)."""

        if side.done:
            return

        quote = self.feed.latest(side.asset_id)
        if quote is None or not quote.is_two_sided or quote.mid is None:
            return  # not enough info yet

        # Pin + safe-mid guards still apply.
        if self._should_skip_side(side_name, side, quote):
            return

        # If somehow we still have a live oid (e.g. previous cancel
        # failed), tear it down before placing again.
        if side.live_oid is not None:
            self._cancel_live(side_name, side)

        target_px = MIN_PX
        # Tiny notional — $1 worth is plenty for a probe. The order won't
        # fill at the floor, so notional only affects the "size" field
        # that appears on the audit log.
        target_qty = max(round(1.0 / target_px, 2), 1.0)
        client_oid = f"te-probe-{self.window.full_slug[:18]}-{side_name}-{int(time.time() * 1000)}"

        try:
            ack = self.router.place_limit(
                symbol=side.asset_id,
                side="buy",
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
            )
        except Exception as e:  # noqa: BLE001
            log.warning("[%s/%s] probe place failed: %s", self.label, side_name, e)
            return

        oid = ack.get("exchange_oid")
        log.info(
            "[%s/%s] PROBE place px=%.2f qty=%.2f oid=%s",
            self.label,
            side_name,
            target_px,
            target_qty,
            oid,
        )

        # Immediate cancel — no wait, no other tick in between.
        if oid:
            try:
                self.router.cancel(oid)
                log.info(
                    "[%s/%s] PROBE cancel ok oid=%s",
                    self.label,
                    side_name,
                    oid,
                )
            except Exception as e:  # noqa: BLE001
                log.warning(
                    "[%s/%s] probe cancel failed (order may sit): %s",
                    self.label,
                    side_name,
                    e,
                )
                # Stash the oid so a subsequent tick can retry the cancel.
                side.live_oid = oid
                side.live_px = target_px
                side.live_qty = target_qty
