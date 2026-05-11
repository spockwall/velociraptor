"""Per-window strategy base class + shared helpers.

A `Strategy` owns the YES/NO state for one Polymarket rolling window and
implements four lifecycle methods the engine drives:

  - `tick()`           — one quote-loop iteration (idempotent).
  - `cancel_all()`     — pull every live order off the book (best-effort).
  - `on_order_update(ev)` — reconcile from a user-channel OrderUpdate.
  - `on_fill(ev)`        — reconcile from a user-channel Fill.

Concrete strategies live alongside this file (`probe.py`, `fill_once.py`,
`one_shot.py`). To add another, subclass `Strategy`, implement
`_target_px(quote)` (and optionally override `_tick_one_side` for special
control flow), then register the class in `strategies/__init__.py`.
"""

from __future__ import annotations

import abc
import dataclasses
import logging
import time
from typing import Optional

from ...io import OrderRouter
from ...market import MarketFeed, MarketWindow, Quote

log = logging.getLogger(__name__)

# Polymarket prices are 0–1; tick size is 0.01.
TICK = 0.01
MIN_PX = 0.01
MAX_PX = 0.99
# Skip-quote threshold: if a side's best_bid sits at or below this price,
# the market has effectively "pinned" that side at the floor and we don't
# want to bid or ask.
PIN_PX = 0.01
# Safe-mid band: when a token's mid falls outside [SAFE_MID_LOW, SAFE_MID_HIGH],
# probe strategies skip placing on it. Above 0.7 means the side is heavily
# favoured and a "far-from-touch" bid could still fill if the spread is wide;
# below 0.3 means the side is unloved and effectively dead.
SAFE_MID_LOW = 0.30
SAFE_MID_HIGH = 0.70


@dataclasses.dataclass
class SideState:
    """One token side (YES or NO) of one window."""

    asset_id: str
    live_oid: Optional[str] = None
    live_px: Optional[float] = None
    live_qty: Optional[float] = None
    filled_this_window: bool = False
    done: bool = False  # one_shot: terminal state for this side, no more action

    def reset(self) -> None:
        self.live_oid = None
        self.live_px = None
        self.live_qty = None


def clamp_px(p: float) -> float:
    p = round(p / TICK) * TICK
    return max(MIN_PX, min(MAX_PX, round(p, 2)))


def qty_for_notional(target_usd: float, px: float) -> float:
    """Polymarket size is in tokens (= USD at $1 settlement). Notional = px * qty."""
    if px <= 0:
        return 0.0
    return round(target_usd / px, 2)


class Strategy(abc.ABC):
    """Abstract base for all per-window strategies."""

    #: Stable name used by the registry / CLI. Subclasses must override.
    name: str = "abstract"

    def __init__(
        self,
        window: MarketWindow,
        router: OrderRouter,
        feed: MarketFeed,
        order_notional_usd: float = 10.0,
        pin_px: float = PIN_PX,
        safe_mid_low: float = SAFE_MID_LOW,
        safe_mid_high: float = SAFE_MID_HIGH,
    ):
        self.window = window
        self.router = router
        self.feed = feed
        self.order_notional_usd = order_notional_usd
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high

        self.yes = SideState(asset_id=window.up_asset_id)
        self.no = SideState(asset_id=window.down_asset_id)

    @property
    def label(self) -> str:
        return self.window.full_slug

    # ── lifecycle (engine calls these) ──

    def tick(self) -> None:
        """One iteration of the quote loop. Idempotent."""
        for side_name, side in (("yes", self.yes), ("no", self.no)):
            self._tick_one_side(side_name, side)

    def cancel_all(self) -> None:
        """Cancel every live order we know about. Best-effort."""
        for side in (self.yes, self.no):
            if side.live_oid is None:
                continue
            try:
                self.router.cancel(side.live_oid)
            except Exception as e:  # noqa: BLE001
                log.warning("[%s] cancel %s failed: %s", self.label, side.live_oid, e)
            side.reset()

    def on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        for side_name, side in (("yes", self.yes), ("no", self.no)):
            if side.live_oid == oid and status in {
                "canceled",
                "filled",
                "rejected",
                "expired",
            }:
                log.info(
                    "[%s/%s] order terminal: status=%s oid=%s",
                    self.label,
                    side_name,
                    status,
                    oid,
                )
                side.reset()

    def on_fill(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        for side_name, side in (("yes", self.yes), ("no", self.no)):
            if side.live_oid == oid or side.asset_id == ev.get("symbol"):
                side.filled_this_window = True
                log.info(
                    "[%s/%s] FILL px=%s qty=%s",
                    self.label,
                    side_name,
                    ev.get("px"),
                    ev.get("qty"),
                )

    # ── extension points (subclasses must / may override) ──

    @abc.abstractmethod
    def _target_px(self, quote: Quote) -> Optional[float]:
        """Compute the target limit price for the current quote. Return None
        to skip placing this tick."""

    def _should_skip_side(self, side_name: str, side: SideState, quote: Quote) -> bool:
        """Returns True when this side should not be placed-on this tick.
        Subclasses can extend by overriding `_extra_skip_reason`."""
        # Pinned-floor guard.
        if quote.best_bid is not None and quote.best_bid <= self.pin_px + 1e-9:
            if side.live_oid is not None:
                self._cancel_live(side_name, side)
            return True
        # Safe-mid band guard.
        if quote.mid is None or quote.mid < self.safe_mid_low or quote.mid > self.safe_mid_high:
            if side.live_oid is not None:
                log.info(
                    "[%s/%s] safe-mid skip (mid=%s outside [%.2f, %.2f]) — cancelling resting",
                    self.label,
                    side_name,
                    f"{quote.mid:.3f}" if quote.mid is not None else "—",
                    self.safe_mid_low,
                    self.safe_mid_high,
                )
                self._cancel_live(side_name, side)
            return True
        return self._extra_skip_reason(side_name, side, quote)

    def _extra_skip_reason(self, side_name: str, side: SideState, quote: Quote) -> bool:
        """Hook for subclass-specific skip rules. Default: never skip."""
        return False

    # ── shared tick body ──

    def _tick_one_side(self, side_name: str, side: SideState) -> None:
        if side.done:
            return

        quote = self.feed.latest(side.asset_id)
        if quote is None or not quote.is_two_sided or quote.mid is None:
            return  # not enough info yet

        if self._should_skip_side(side_name, side, quote):
            return

        target_px = self._target_px(quote)
        if target_px is None:
            return
        target_qty = qty_for_notional(self.order_notional_usd, target_px)
        if target_qty <= 0:
            return

        # Idempotent: live quote already matches target → no-op.
        if (
            side.live_oid is not None
            and side.live_px == target_px
            and side.live_qty == target_qty
        ):
            return

        # Cancel any stale resting order first, then place fresh.
        if side.live_oid is not None:
            self._cancel_live(side_name, side)

        client_oid = f"te-{self.window.full_slug[:18]}-{side_name}-{int(time.time())}"
        try:
            ack = self.router.place_limit(
                symbol=side.asset_id,
                side="buy",  # always quote the buy side for now
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
            )
        except Exception as e:  # noqa: BLE001
            log.warning("[%s/%s] place failed: %s", self.label, side_name, e)
            return

        side.live_oid = ack.get("exchange_oid")
        side.live_px = target_px
        side.live_qty = target_qty
        log.info(
            "[%s/%s] PLACE px=%.2f qty=%.2f oid=%s",
            self.label,
            side_name,
            target_px,
            target_qty,
            side.live_oid,
        )
        self._after_place(side_name, side)

    def _after_place(self, side_name: str, side: SideState) -> None:
        """Hook called after a successful place. Subclasses use this to mark
        sides done (one_shot) or trigger immediate cancel."""

    def _cancel_live(self, side_name: str, side: SideState) -> None:
        """Cancel `side.live_oid` and reset SideState. Soft-fails."""
        if side.live_oid is None:
            return
        try:
            self.router.cancel(side.live_oid)
        except Exception as e:  # noqa: BLE001
            log.warning(
                "[%s/%s] stale cancel failed (will overwrite anyway): %s",
                self.label,
                side_name,
                e,
            )
        side.reset()
