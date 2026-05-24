"""Window discovery: poll the backend, build window objects, filter.

Two layers live here:

  - **raw discovery** (`discover` / `discover_kalshi`, with the
    `group_into_windows` helper): hit the backend HTTP API and turn the JSON
    into `PolymarketWindow` / `KalshiWindow` objects (defined in
    `typings/window.py`).
  - **active helpers** (`active_polymarket_windows` /
    `active_kalshi_windows`): wrap discovery with the bookkeeping every
    caller used to inline — swallow+log errors (returning **None**, vs `[]`
    for "answered, nothing live") and drop already-ended windows.

The engine (`app.py`) and the observer call the active helpers; what they
DO with the windows — manage strategies vs. manage subscriptions — stays
in each of them.
"""

from __future__ import annotations

import logging
import time
from typing import Iterable, Optional

import requests

from ..typings.window import KalshiWindow, PolymarketWindow

log = logging.getLogger(__name__)

DEFAULT_BACKEND = "http://127.0.0.1:3000"


# ── Polymarket raw discovery ─────────────────────────────────────────────────


def group_into_windows(raw: Iterable[dict]) -> list[PolymarketWindow]:
    """Pair the up/down rows for each (base_slug, full_slug) into a
    `PolymarketWindow`."""
    by_full: dict[str, dict[str, dict]] = {}
    for m in raw:
        key = m["full_slug"]
        by_full.setdefault(key, {})[m["side"]] = m
    out: list[PolymarketWindow] = []
    for full_slug, sides in by_full.items():
        up, down = sides.get("up"), sides.get("down")
        if up is None or down is None:
            log.debug(f"skipping {full_slug} — only one side resolved")
            continue
        out.append(
            PolymarketWindow(
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


def discover_polymarket(
    base_slugs: list[str],
    backend_url: str = DEFAULT_BACKEND,
) -> list[PolymarketWindow]:
    """All currently-listed windows whose `base_slug` is in `base_slugs`.

    Fetches the raw `/api/polymarket/markets` rows (one per side) and pairs
    them into windows."""
    r = requests.get(f"{backend_url}/api/polymarket/markets", timeout=5)
    r.raise_for_status()
    windows = group_into_windows(r.json())
    return [w for w in windows if w.base_slug in base_slugs]


# ── Kalshi raw discovery ─────────────────────────────────────────────────────


def discover_kalshi(
    series: list[str],
    backend_url: str = DEFAULT_BACKEND,
) -> list[KalshiWindow]:
    """Listed Kalshi tickers whose `series` is in `series`.

    Fetches the raw `/api/kalshi/markets` rows directly."""
    r = requests.get(f"{backend_url}/api/kalshi/markets", timeout=5)
    r.raise_for_status()
    raw = r.json()
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


# ── Active helpers (error + expiry handling) ─────────────────────────────────


def active_polymarket_windows(
    base_slugs: list[str],
    backend_url: str = DEFAULT_BACKEND,
    *,
    drop_expired: bool = True,
) -> Optional[list[PolymarketWindow]]:
    """Live Polymarket windows for `base_slugs`, or `None` if discovery
    failed. Drops already-ended windows unless `drop_expired=False`."""
    try:
        windows = discover_polymarket(base_slugs, backend_url)
    except Exception as e:  # noqa: BLE001
        log.warning(f"polymarket discover failed: {e}")
        return None
    return _drop_expired(windows) if drop_expired else windows


def active_kalshi_windows(
    series: list[str],
    backend_url: str = DEFAULT_BACKEND,
    *,
    drop_expired: bool = True,
) -> Optional[list[KalshiWindow]]:
    """Live Kalshi windows for `series`, or `None` if discovery failed.
    Same expiry handling as `active_polymarket_windows`."""
    try:
        windows = discover_kalshi(series, backend_url)
    except Exception as e:  # noqa: BLE001
        log.warning(f"kalshi discover failed: {e}")
        return None
    return _drop_expired(windows) if drop_expired else windows


def _drop_expired(windows: list) -> list:
    now = int(time.time())
    return [w for w in windows if w.window_end > now]
