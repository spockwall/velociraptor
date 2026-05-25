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

    Only `base_slug` is required. `full_slug` is updated on each
    rollover (the payload `symbol` field IS the full_slug under the
    current wire contract).

    `up_asset_id` / `down_asset_id` are **deprecated** here — the
    canonical source for per-token asset_ids is
    `MarketState.asset_ids(full_slug)`, populated by strategies on
    rollover via `market.fetch_asset_ids`. The fields stay for one
    transitional cycle so any straggling reader doesn't crash; new
    code should read via `MarketState`.
    """

    base_slug: str  # e.g. "btc-updown-15m"
    full_slug: typing.Optional[str] = None
    window_start: typing.Optional[int] = None
    interval_secs: typing.Optional[int] = None
    up_asset_id: typing.Optional[str] = None  # deprecated; see MarketState
    down_asset_id: typing.Optional[str] = None  # deprecated; see MarketState

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
