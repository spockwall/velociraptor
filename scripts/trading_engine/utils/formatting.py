"""Status-line formatting helpers for the observe's snapshot table.

Pure presentation: turn a `Quote` / `Trade` into one aligned status row.
Kept out of `observe.py` so the formatting can be reused and unit-tested
without the observe's threading/subscription machinery, and decoupled
from `TrackedSymbol` — `format_row` takes the label + optional window end
directly rather than the tracker object.
"""

from __future__ import annotations

import time
from typing import Optional

from ..market import Quote, Trade


def format_trade(trade: Optional[Trade], now_ns: int) -> str:
    """One-line `last=…` fragment for a Trade (or a placeholder)."""
    if trade is None:
        return "last=          —"
    t_age_ms = (now_ns - trade.received_ns) / 1e6
    px = f"{trade.price:.6f}"
    return f"last={px:>10s}x{trade.size:<8.4g}{trade.side:<4s} ({t_age_ms:6.0f}ms)"


def format_row(
    label: str,
    quote: Optional[Quote],
    trade: Optional[Trade],
    now_ns: int,
    window_end: Optional[int] = None,
) -> str:
    """One aligned status row: bid / ask / mid / age + last trade, and a
    `win-end-in=…` suffix when `window_end` is given (rotating markets)."""
    tr = format_trade(trade, now_ns)
    if quote is None:
        return f"{label:48s}  (no quote yet)  {tr}"
    age_ms = (now_ns - quote.received_ns) / 1e6
    bid = f"{quote.best_bid:.6f}" if quote.best_bid is not None else "—"
    ask = f"{quote.best_ask:.6f}" if quote.best_ask is not None else "—"
    mid = f"{quote.mid:.6f}" if quote.mid is not None else "—"
    age = f"{age_ms:7.0f}ms"
    win = ""
    if window_end is not None:
        remaining = window_end - int(time.time())
        win = f"  win-end-in={remaining:>4d}s"
    return (
        f"{label:48s}  bid={bid:>10s}  ask={ask:>10s}  mid={mid:>10s}  "
        f"age={age}  {tr}{win}"
    )
