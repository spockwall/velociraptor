"""Fill-once strategy — cross the spread, fill exactly once per side per window.

Quotes ask + N ticks (crosses) so the order takes immediately. After a fill
arrives on a side, that side is parked until the next window. Used to be
`--step 2`.
"""

from __future__ import annotations

from typing import Optional

from ...market import Quote
from .base import Strategy, SideState, TICK, clamp_px


class FillOnceStrategy(Strategy):
    name = "fill_once"

    def __init__(self, *args, tight_offset_ticks: int = 1, **kwargs):
        super().__init__(*args, **kwargs)
        self.tight_offset_ticks = tight_offset_ticks

    def _target_px(self, quote: Quote) -> Optional[float]:
        if quote.best_ask is None:
            return None
        return clamp_px(quote.best_ask + self.tight_offset_ticks * TICK)

    def _extra_skip_reason(self, side_name: str, side: SideState, quote: Quote) -> bool:
        # Stop quoting a side once it has filled this window.
        return side.filled_this_window
