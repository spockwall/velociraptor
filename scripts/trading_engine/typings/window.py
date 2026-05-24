"""Rolling-window data types.

Pure dataclasses for the two market families the engine tracks:

  - `PolymarketWindow` — one up/down window, holding both YES (up) and NO
    (down) token ids.
  - `KalshiWindow`     — one Kalshi market window (single ticker; YES/NO is
    carried inside the order, not split across two tokens).

Both expose `window_end` (= window_start + interval_secs) so callers can
do a pure clock check for rotation without re-querying the backend. The
discovery functions that build these from the backend live in
`utils/windows.py`.
"""

from __future__ import annotations

import dataclasses
import typing


@dataclasses.dataclass
class PolymarketWindow:
    """One rolling Polymarket window.

    Only `base_slug` is required — the rest is populated from incoming
    snapshot payloads (the server stamps `full_slug` per frame, and the
    payload `symbol` IS the current up_asset_id). `down_asset_id` is
    intentionally unused now that the server forwards only the UP side
    (down is the mirror image of the same orderbook). Kept as an
    `Optional[str]` field for back-compat with code that imports it.
    """

    base_slug: str  # e.g. "btc-updown-15m"
    full_slug: typing.Optional[str] = None  # filled from payload on first snapshot
    window_start: typing.Optional[int] = None
    interval_secs: typing.Optional[int] = None
    up_asset_id: typing.Optional[str] = None
    down_asset_id: typing.Optional[str] = (
        None  # vestigial — never set under static-topic forwarding
    )

    @property
    def window_end(self) -> typing.Optional[int]:
        if self.window_start is None or self.interval_secs is None:
            return None
        return self.window_start + self.interval_secs


@dataclasses.dataclass
class KalshiWindow:
    """One Kalshi market window. Kalshi exposes a single `ticker` per market
    (no YES/NO split — orders carry side internally)."""

    series: str  # e.g. "KXBTC15M"
    ticker: str  # full Kalshi ticker (also the ZMQ symbol)
    window_start: int  # UTC seconds
    interval_secs: int

    @property
    def window_end(self) -> int:
        return self.window_start + self.interval_secs
