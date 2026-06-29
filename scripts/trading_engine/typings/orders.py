"""Typed views over the executor's `OrderAck` / `FillInfo` wire shapes.

Mirrors the Rust `libs/src/protocol/orders.rs` structs that the executor
msgpack-encodes into `OrderResponse.result.Ok.Ack`:

    OrderAck { client_oid, exchange_oid, status, ex_timestamp, recv_timestamp, fill? }
    FillInfo { making_amount, taking_amount, success, error_msg?,
               transaction_hashes[], trade_ids[] }

The `fill` block is populated only on the Polymarket CLOB place path (it
echoes the venue's `PostOrderResponse`); other exchanges/paths leave it
absent. Both parsers are total and forgiving: unknown/missing keys fall back
to sensible defaults so a protocol addition never crashes the engine.

These are *views*, not the transport — `ExecutorClient` still returns raw
dicts. Use `OrderAck.from_wire(ack_dict)` when a strategy wants typed access
to the immediate fill (e.g. how much a market order matched at ack time).
"""

from __future__ import annotations

import dataclasses
from typing import Any, Optional


def _as_float(v: Any) -> float:
    try:
        return float(v)
    except (TypeError, ValueError):
        return 0.0


@dataclasses.dataclass(frozen=True)
class FillInfo:
    """Immediate fill/settlement detail echoed from the exchange ack.

    For a Polymarket market order, `making_amount` / `taking_amount` are the
    amounts matched synchronously when the order was posted (status
    `matched`); `transaction_hashes` are the on-chain settlement txs.
    """

    making_amount: float = 0.0
    taking_amount: float = 0.0
    success: bool = False
    error_msg: Optional[str] = None
    transaction_hashes: tuple[str, ...] = ()
    trade_ids: tuple[str, ...] = ()

    @classmethod
    def from_wire(cls, d: Any) -> Optional["FillInfo"]:
        """Parse the `fill` sub-map of an OrderAck. Returns `None` when the
        ack carried no fill block (absent key or explicit nil)."""
        if not isinstance(d, dict):
            return None
        msg = d.get("error_msg")
        return cls(
            making_amount=_as_float(d.get("making_amount")),
            taking_amount=_as_float(d.get("taking_amount")),
            success=bool(d.get("success", False)),
            error_msg=(msg if isinstance(msg, str) and msg else None),
            transaction_hashes=tuple(
                str(h) for h in (d.get("transaction_hashes") or [])
            ),
            trade_ids=tuple(str(t) for t in (d.get("trade_ids") or [])),
        )

    @property
    def matched(self) -> bool:
        """True when the venue reported any synchronous fill."""
        return self.making_amount > 0.0 or self.taking_amount > 0.0


@dataclasses.dataclass(frozen=True)
class OrderAck:
    """Typed view over the executor's `OrderAck`."""

    client_oid: str = ""
    exchange_oid: str = ""
    status: str = ""
    # Exchange-stamped ack time in Unix ns (`0` when the venue's ack carries
    # no timestamp — true for Polymarket CLOB and Kalshi today).
    ex_timestamp: int = 0
    # Executor-side time the ack was constructed, in Unix ns.
    recv_timestamp: int = 0
    fill: Optional[FillInfo] = None

    @classmethod
    def from_wire(cls, d: Any) -> "OrderAck":
        """Parse the inner Ack dict (i.e. `unwrap(resp)` on a place reply)."""
        if not isinstance(d, dict):
            return cls()
        ex = d.get("ex_timestamp")
        recv = d.get("recv_timestamp")
        return cls(
            client_oid=str(d.get("client_oid") or ""),
            exchange_oid=str(d.get("exchange_oid") or ""),
            status=str(d.get("status") or ""),
            ex_timestamp=int(ex) if isinstance(ex, (int, float)) else 0,
            recv_timestamp=int(recv) if isinstance(recv, (int, float)) else 0,
            fill=FillInfo.from_wire(d.get("fill")),
        )
