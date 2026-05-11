"""Resolve current rolling windows from the backend.

Two markets:
  - Polymarket up-down — `/api/polymarket/markets` → MarketWindow(up,down)
  - Kalshi 15m series — `/api/kalshi/markets`     → KalshiWindow(ticker)

Both pair (base_id, full_id, window_start) so the engine can detect
rotation cleanly and subscribe to the right ZMQ topics.
"""

from __future__ import annotations

import dataclasses
import logging
from typing import Iterable

import requests

log = logging.getLogger(__name__)


@dataclasses.dataclass
class MarketWindow:
    """One rolling window. Holds both YES (up) and NO (down) token ids."""

    base_slug: str          # e.g. "btc-updown-15m"
    full_slug: str          # e.g. "btc-updown-15m-1715423400"
    window_start: int       # UTC seconds
    interval_secs: int
    up_asset_id: str        # YES side token
    down_asset_id: str      # NO side token

    @property
    def window_end(self) -> int:
        return self.window_start + self.interval_secs


def fetch_markets(backend_url: str = "http://127.0.0.1:3000") -> list[dict]:
    """Raw `/api/polymarket/markets` response. Each entry is one side of one window."""
    r = requests.get(f"{backend_url}/api/polymarket/markets", timeout=5)
    r.raise_for_status()
    return r.json()


def group_into_windows(raw: Iterable[dict]) -> list[MarketWindow]:
    """Pair the up/down rows for each (base_slug, full_slug) into MarketWindow."""
    by_full: dict[str, dict[str, dict]] = {}
    for m in raw:
        key = m["full_slug"]
        by_full.setdefault(key, {})[m["side"]] = m
    out: list[MarketWindow] = []
    for full_slug, sides in by_full.items():
        up, down = sides.get("up"), sides.get("down")
        if up is None or down is None:
            log.debug("skipping %s — only one side resolved", full_slug)
            continue
        out.append(
            MarketWindow(
                base_slug=up["base_slug"],
                full_slug=full_slug,
                window_start=up["window_start"],
                interval_secs=up["interval_secs"],
                up_asset_id=up["asset_id"],
                down_asset_id=down["asset_id"],
            )
        )
    out.sort(key=lambda w: (w.base_slug, w.window_start))
    return out


def discover(
    base_slugs: list[str],
    backend_url: str = "http://127.0.0.1:3000",
) -> list[MarketWindow]:
    """All currently-active windows whose `base_slug` is in `base_slugs`."""
    raw = fetch_markets(backend_url)
    windows = group_into_windows(raw)
    return [w for w in windows if w.base_slug in base_slugs]


# ── Kalshi ──────────────────────────────────────────────────────────────────


@dataclasses.dataclass
class KalshiWindow:
    """One Kalshi market window. Kalshi exposes a single `ticker` per market
    (no YES/NO split — orders carry side internally)."""

    series: str             # e.g. "KXBTC15M"
    ticker: str             # full Kalshi ticker (also the ZMQ symbol)
    window_start: int       # UTC seconds
    interval_secs: int

    @property
    def window_end(self) -> int:
        return self.window_start + self.interval_secs


def fetch_kalshi_markets(backend_url: str = "http://127.0.0.1:3000") -> list[dict]:
    """Raw `/api/kalshi/markets` response."""
    r = requests.get(f"{backend_url}/api/kalshi/markets", timeout=5)
    r.raise_for_status()
    return r.json()


def discover_kalshi(
    series: list[str],
    backend_url: str = "http://127.0.0.1:3000",
) -> list[KalshiWindow]:
    """Active Kalshi tickers whose `series` is in `series`."""
    raw = fetch_kalshi_markets(backend_url)
    out: list[KalshiWindow] = []
    for m in raw:
        if m.get("series") not in series:
            continue
        out.append(
            KalshiWindow(
                series=m["series"],
                ticker=m["ticker"],
                window_start=int(m.get("window_start") or 0),
                interval_secs=int(m.get("interval_secs") or 0),
            )
        )
    out.sort(key=lambda w: (w.series, w.window_start))
    return out
