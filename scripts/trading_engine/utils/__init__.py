"""Shared, dependency-light helpers for the trading engine.

- `formatting`: status-line formatters for the observer's snapshot table.
- `windows`:    active Polymarket/Kalshi window discovery helpers.
"""

from __future__ import annotations

from .formatting import format_row, format_trade
from .windows import (
    active_kalshi_windows,
    active_polymarket_windows,
    discover_polymarket,
    discover_kalshi,
    group_into_windows,
)

__all__ = [
    "active_kalshi_windows",
    "active_polymarket_windows",
    "discover_polymarket",
    "discover_kalshi",
    "format_row",
    "format_trade",
    "group_into_windows",
]
