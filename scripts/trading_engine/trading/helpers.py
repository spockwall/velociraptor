"""Shared constants + utility functions for Polymarket strategies.

Lives at the `trading/` level (not under `strategies/`) because the
helpers are venue-shape utilities, not strategy machinery: any caller
that touches Polymarket prices (a strategy, the dispatcher, a future
diagnostics tool) can import them without crossing into the strategy
package. The base class deliberately doesn't re-export them — each
strategy imports what it needs.
"""

from __future__ import annotations

from typing import Optional

from ..market import MarketState

# Polymarket prices are 0–1; tick size is 0.01.
TICK = 0.01
MIN_PX = 0.01
MAX_PX = 0.99

# Skip-quote threshold: if best_bid sits at or below this price, the
# market has effectively "pinned" the side at the floor and we don't
# want to bid or ask.
PIN_PX = 0.01

# Safe-mid band: when the token's mid falls outside [LOW, HIGH], probe
# strategies skip placing on it. Above 0.7 means the side is heavily
# favoured and a "far-from-touch" bid could still fill if the spread is
# wide; below 0.3 means the side is unloved and effectively dead.
SAFE_MID_LOW = 0.30
SAFE_MID_HIGH = 0.70


def clamp_px(p: float) -> float:
    """Round to the Polymarket tick grid and clip to [MIN_PX, MAX_PX]."""
    p = round(p / TICK) * TICK
    return max(MIN_PX, min(MAX_PX, round(p, 2)))


def qty_for_notional(target_usd: float, px: float) -> float:
    """Polymarket size is in tokens (= USD at $1 settlement).
    Notional = px * qty."""
    if px <= 0:
        return 0.0
    return round(target_usd / px, 2)


def safe_mid_guard(
    market: MarketState,
    exchange: str,
    symbol: str,
    *,
    low: float = SAFE_MID_LOW,
    high: float = SAFE_MID_HIGH,
    pin_px: float = PIN_PX,
) -> Optional[str]:
    """Return None if the latest quote is safe to act on, else a short
    reason string. Strategies call this at the top of their quote
    callback and bail when it returns non-None."""
    q = market.quote(exchange, symbol)
    if q is None or not q.is_two_sided or q.mid is None:
        return "no-quote"
    if q.best_bid is not None and q.best_bid <= pin_px + 1e-9:
        return "pinned"
    if q.mid < low or q.mid > high:
        return f"mid-{q.mid:.3f}-out-of-[{low:.2f},{high:.2f}]"
    return None
