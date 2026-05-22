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


@dataclasses.dataclass
class PolymarketWindow:
    """One rolling Polymarket window. Holds both YES (up) and NO (down)
    token ids."""

    base_slug: str          # e.g. "btc-updown-15m"
    full_slug: str          # e.g. "btc-updown-15m-1715423400"
    window_start: int       # UTC seconds
    interval_secs: int
    up_asset_id: str        # YES side token
    down_asset_id: str      # NO side token

    @property
    def window_end(self) -> int:
        return self.window_start + self.interval_secs


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
