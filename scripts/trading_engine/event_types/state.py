"""Per-strategy state container.

Every `Strategy` (see `strategies/base.py`) owns one `StrategyState`. It
gives a strategy a clean, first-class place to record its own history
instead of scattering ad-hoc instance attributes (the way the old
`momentum` strategy kept `self._samples` / `self.entry_oid` directly).

Three pieces:

  - `trades`  — a bounded deque of the `Trade` events the strategy chose
                to record (e.g. "the last 30 trades"). Bounded so a
                long-running window can't grow memory without limit.
  - `orders`  — an `OrderLedger`: every order the strategy placed this
                window, with its lifecycle status. Answers questions like
                "how many orders have I opened?" and "what's still live?".
  - `scratch` — a free-form `dict` for anything else a strategy wants to
                stash (rolling samples, latches, counters). Kept as plain
                key/value so new strategies don't need a new field here.

State is **in-memory only** and lives for the lifetime of the strategy
instance — which is one rolling window. It is intentionally NOT persisted
across restarts (a restarted engine has no claim on a previous run's
orders; see the README "Known limitations"). `__repr__` is implemented so
the state is easy to eyeball in a debugger or log line.
"""

from __future__ import annotations

import dataclasses
from collections import deque
from typing import Any, Deque, Optional

# Terminal order states (mirrors the user-channel `order_update` statuses
# the engine already reconciles on — see strategies/base.py on_order_update).
_TERMINAL = {"filled", "canceled", "rejected", "expired"}


@dataclasses.dataclass
class OrderRecord:
    """One order this strategy placed.

    Lifecycle: ``placed`` → ``live`` → (``filled`` | ``canceled`` |
    ``rejected`` | ``expired``). ``exchange_oid`` is None until the place
    ack lands, so the ledger keys on ``client_oid`` until then.
    """

    client_oid: str
    side_name: str            # free-form: "yes" / "no" / "yes-exit" / ...
    symbol: str
    direction: str            # "buy" | "sell"
    px: float
    qty: float
    exchange_oid: Optional[str] = None
    status: str = "placed"
    filled_qty: float = 0.0

    @property
    def is_terminal(self) -> bool:
        return self.status in _TERMINAL


class OrderLedger:
    """All orders a strategy placed this window.

    Keyed by ``exchange_oid`` once known, falling back to ``client_oid``
    until the place ack binds the exchange id. Lookups accept either id.
    """

    def __init__(self) -> None:
        # Single dict keyed by whatever id we currently know the order by.
        # When the exchange_oid arrives we re-key from client_oid to it.
        self._by_id: dict[str, OrderRecord] = {}
        self._opened = 0

    def add(self, rec: OrderRecord) -> None:
        key = rec.exchange_oid or rec.client_oid
        self._by_id[key] = rec
        self._opened += 1

    def bind_exchange_oid(self, client_oid: str, exchange_oid: str) -> None:
        """Re-key a record from its client_oid to the exchange_oid the
        place ack returned. No-op if the client_oid isn't tracked."""
        rec = self._by_id.pop(client_oid, None)
        if rec is None:
            return
        rec.exchange_oid = exchange_oid
        self._by_id[exchange_oid] = rec

    def get(self, order_id: str) -> Optional[OrderRecord]:
        """Look up by exchange_oid or client_oid."""
        return self._by_id.get(order_id)

    def mark(self, order_id: str, status: str, filled_qty: float = 0.0) -> None:
        """Update an order's status. No-op if unknown."""
        rec = self._by_id.get(order_id)
        if rec is None:
            return
        rec.status = status
        if filled_qty:
            rec.filled_qty = filled_qty

    def live(self) -> list[OrderRecord]:
        """Orders that are placed or resting (not terminal)."""
        return [r for r in self._by_id.values() if not r.is_terminal]

    def count_opened(self) -> int:
        """Total orders ever placed this window (terminal ones included)."""
        return self._opened

    def __repr__(self) -> str:
        return (
            f"OrderLedger(opened={self._opened}, live={len(self.live())})"
        )


class StrategyState:
    """First-class per-strategy state. One instance per strategy (= one
    rolling window). See module docstring."""

    def __init__(self, trade_history: int = 30) -> None:
        self.trades: Deque = deque(maxlen=trade_history)
        self.orders = OrderLedger()
        self.scratch: dict[str, Any] = {}

    def record_trade(self, trade: Any) -> None:
        """Append a `Trade` to the bounded history (oldest dropped)."""
        self.trades.append(trade)

    def __repr__(self) -> str:
        return (
            f"StrategyState(trades={len(self.trades)}, "
            f"orders={self.orders!r}, scratch={sorted(self.scratch)})"
        )
