"""Shared, dependency-light helpers for the trading engine.

  - `formatting`: status-line formatters for the observer's snapshot table.
"""

from __future__ import annotations

from .formatting import format_row, format_trade

__all__ = [
    "format_row",
    "format_trade",
]
