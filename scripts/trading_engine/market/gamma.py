"""Polymarket Gamma REST + slug helpers.

Used by the trading engine to:
  - resolve a window's `clobTokenIds` from its `full_slug`
    (`fetch_asset_ids`),
  - compute the current window's `full_slug` from `base_slug` + clock
    (`current_full_slug` / `interval_secs_from_base_slug`) at startup
    so the engine has asset_ids in hand before the first quote
    arrives.

Gamma is public + free; 10s timeout per call; failures return `None`
and the caller should refuse to trade until a later attempt succeeds.
"""

from __future__ import annotations

import json
import logging
import re
import time
import urllib.error
import urllib.request
from typing import Optional

log = logging.getLogger(__name__)

# Suffix convention: `*-{N}m` (minutes) or `*-{N}s` (seconds). Matches
# every Polymarket rolling slug currently in `configs/<env>/config.yaml`
# (btc-updown-15m, eth-updown-5m, ...). Anchored at the end so the
# prefix can contain hyphens.
_SUFFIX_RE = re.compile(r"-(\d+)([ms])$")


def interval_secs_from_base_slug(base_slug: str) -> Optional[int]:
    """Parse `*-{N}m` / `*-{N}s` from the end of a Polymarket rolling
    base_slug. Returns `None` if the suffix isn't recognised — caller
    should fall back to a CLI flag or refuse to bootstrap."""
    m = _SUFFIX_RE.search(base_slug)
    if not m:
        return None
    n, unit = int(m.group(1)), m.group(2)
    return n * (60 if unit == "m" else 1)


def current_full_slug(base_slug: str, interval_secs: int) -> str:
    """Compute the full_slug for the current window from `base_slug` +
    clock. Floors `now` to the previous boundary, matching what the
    Polymarket Gamma API expects."""
    now = int(time.time())
    win_start = (now // interval_secs) * interval_secs
    return f"{base_slug}-{win_start}"

_GAMMA_BASE = "https://gamma-api.polymarket.com/markets/slug"
_HEADERS = {"Accept": "application/json", "User-Agent": "Mozilla/5.0"}
_TIMEOUT_S = 10.0


def fetch_asset_ids(full_slug: str) -> Optional[tuple[str, str]]:
    """Return `(up_asset_id, down_asset_id)` for a Polymarket
    rolling-window full_slug, or `None` if the slug isn't open / Gamma
    is unreachable / response shape is unexpected.

    Index 0 of Gamma's `clobTokenIds` is Yes/Up; index 1 is No/Down —
    matches the Rust resolver's convention (`PolymarketLabeledAsset`).
    """
    url = f"{_GAMMA_BASE}/{full_slug}"
    req = urllib.request.Request(url, headers=_HEADERS)
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT_S) as resp:
            body = json.loads(resp.read())
    except urllib.error.HTTPError as e:
        if e.code == 404:
            log.warning("gamma: %s not found (market not open?)", full_slug)
        else:
            log.warning("gamma: %s HTTP %s", full_slug, e.code)
        return None
    except Exception as e:  # noqa: BLE001
        log.warning("gamma: %s failed: %s", full_slug, e)
        return None

    raw = body.get("clobTokenIds")
    if isinstance(raw, str):
        try:
            ids = json.loads(raw)
        except json.JSONDecodeError:
            log.warning("gamma: %s: clobTokenIds is non-JSON string", full_slug)
            return None
    elif isinstance(raw, list):
        ids = raw
    else:
        log.warning("gamma: %s: clobTokenIds missing/unexpected", full_slug)
        return None

    if not isinstance(ids, list) or len(ids) < 2:
        log.warning("gamma: %s: got %d token id(s), need 2", full_slug, len(ids or []))
        return None
    up, down = str(ids[0]), str(ids[1])
    if not up or not down:
        log.warning("gamma: %s: empty token id in response", full_slug)
        return None
    return up, down
