"""One-shot strategy — place once, cancel once, exit.

Per side: place a single buy at the absolute floor ($0.01), then cancel it
synchronously in the same tick. Mark the side done. Once both sides are
done, the engine exits.

Two safety properties (same as `probe`):

1. **Price is the floor** (`MIN_PX = 0.01`). At $0.01 buy, the order is
   below every standing bid, so the matching engine cannot match against
   any resting sell. No partial fills.

2. **Cancel is synchronous and immediate.** No "next tick" gap.

Useful as a one-time round-trip probe through the order ROUTER + user
channel — same shape as the old `scripts/poly_place_cancel.py` but driven
by the engine, so it picks up the current Polymarket window automatically
and runs across both YES and NO without manual token-id editing.
"""

from __future__ import annotations

import logging
import time
from typing import Optional

from ...market import Quote
from .base import MIN_PX, SideState, Strategy

log = logging.getLogger(__name__)


class OneShotStrategy(Strategy):
    name = "one_shot"

    def _target_px(self, quote: Quote) -> Optional[float]:
        return MIN_PX

    def _tick_one_side(self, side_name: str, side: SideState) -> None:
        if side.done:
            return

        quote = self.feed.latest(side.asset_id)
        if quote is None or not quote.is_two_sided or quote.mid is None:
            return  # not enough info yet

        if self._should_skip_side(side_name, side, quote):
            return

        # If something left a live oid (e.g. previous cancel failed), tear it
        # down before placing.
        if side.live_oid is not None:
            self._cancel_live(side_name, side)

        target_px = MIN_PX
        target_qty = max(round(1.0 / target_px, 2), 1.0)
        client_oid = f"te-1shot-{self.window.full_slug[:18]}-{side_name}-{int(time.time() * 1000)}"

        try:
            ack = self.router.place_limit(
                symbol=side.asset_id,
                side="buy",
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
            )
        except Exception as e:  # noqa: BLE001
            log.warning("[%s/%s] one_shot place failed: %s", self.label, side_name, e)
            return

        oid = ack.get("exchange_oid")
        log.info(
            "[%s/%s] ONE_SHOT place px=%.2f qty=%.2f oid=%s",
            self.label,
            side_name,
            target_px,
            target_qty,
            oid,
        )

        if oid:
            try:
                self.router.cancel(oid)
                log.info(
                    "[%s/%s] ONE_SHOT cancel ok oid=%s",
                    self.label,
                    side_name,
                    oid,
                )
                side.done = True
            except Exception as e:  # noqa: BLE001
                log.warning(
                    "[%s/%s] one_shot cancel failed; will retry next tick: %s",
                    self.label,
                    side_name,
                    e,
                )
                # Stash so a retry on the next tick can cancel it before
                # marking the side done.
                side.live_oid = oid
                side.live_px = target_px
                side.live_qty = target_qty
        else:
            # No exchange_oid returned somehow — treat as done so we don't
            # spin forever. Probably means the executor returned an Err that
            # `place_limit` did not raise.
            log.warning(
                "[%s/%s] one_shot: place ACK had no exchange_oid; finalising anyway",
                self.label,
                side_name,
            )
            side.done = True

    @property
    def is_done(self) -> bool:
        return self.yes.done and self.no.done
