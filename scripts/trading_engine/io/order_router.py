"""Thin wrapper around the low-level `ExecutorClient` (`io.executor_client`).

Adds a single helper class with stable semantics for the strategy loop:

- `place_limit(...)` → returns the exchange_oid (or raises on Err)
- `cancel(exchange_oid)` → returns count cancelled (0 if already gone)
- `cancel_all()` → for clean shutdown
- `heartbeat()` → diagnostic round-trip; returns the inner Ack dict

If an `EventLog` is supplied, every successful (and failed) call is recorded
to disk for research. Strategies pass their own `strategy=` / `label=` so the
log row carries attribution.

This module also doubles as the heartbeat CLI when run as a module:

    python -m scripts.trading_engine.io.order_router            # 3 heartbeats
    python -m scripts.trading_engine.io.order_router --count 10
    python -m scripts.trading_engine.io.order_router --endpoint tcp://10.0.0.5:5557

That replaces the standalone `scripts/poly_heartbeat.py` script.
"""

from __future__ import annotations

import json
import logging
import time
from typing import Optional

from ..typings.orders import FillInfo
from .event_log import EventLog
from .executor_client import (
    ExecutorClient,
    cancel as _cancel_req,
    cancel_all as _cancel_all_req,
    heartbeat as _heartbeat_req,
    is_ok,
    place as _place_req,
    unwrap,
)

log = logging.getLogger(__name__)


def _fill_extra(fill: Optional[FillInfo]) -> dict:
    """Project a parsed `FillInfo` onto the event-log `extra` columns.
    Empty dict when there was no fill block (non-Polymarket / no match)."""
    if fill is None:
        return {}
    extra: dict = {
        "making_amount": fill.making_amount,
        "taking_amount": fill.taking_amount,
        "fill_success": fill.success,
    }
    if fill.trade_ids:
        extra["trade_ids"] = ",".join(fill.trade_ids)
    if fill.transaction_hashes:
        extra["tx_hashes"] = ",".join(fill.transaction_hashes)
    return extra


def _log_fill(kind: str, label: Optional[str], ack: dict) -> None:
    """Emit one INFO line with the full ack `fill` block for every order
    placement (limit or market), so all strategies surface the venue's
    synchronous fill detail uniformly. No-op when the ack carried no fill
    (non-Polymarket path / nothing matched at ack time)."""
    fill = ack.get("fill")
    if fill is None:
        return
    tag = f"[{label}] " if label else ""
    log.info(
        f"{tag}{kind} ack "
        f"oid={ack.get('exchange_oid')} status={ack.get('status')} "
        f"fill={json.dumps(fill, default=str)}"
    )


class OrderRouter:
    def __init__(
        self,
        endpoint: str = "tcp://127.0.0.1:5557",
        exchange: str = "polymarket",
        timeout_ms: int = 10_000,
        event_log: Optional[EventLog] = None,
    ):
        self._endpoint = endpoint
        self._exchange = exchange
        self._cli = ExecutorClient(endpoint, timeout_ms=timeout_ms)
        self._event_log = event_log

    # ── lifecycle ──

    def __enter__(self) -> "OrderRouter":
        self._cli.connect()
        return self

    def __exit__(self, *_exc: object) -> None:
        self._cli.close()

    # ── orders ──

    def place_limit(
        self,
        symbol: str,
        side: str,
        px: float,
        qty: float,
        client_oid: Optional[str] = None,
        tif: str = "GTC",
        # Attribution for the event log; ignored when no EventLog attached.
        strategy: Optional[str] = None,
        label: Optional[str] = None,
    ) -> dict:
        """Place a GTC limit. Returns the inner OrderAck dict on success.
        Raises RuntimeError on Err.

        The ack dict carries an optional `fill` sub-map on the Polymarket
        path (the venue's `PostOrderResponse`): matched maker/taker amounts,
        settlement tx hashes, trade ids. Parse it with
        `typings.orders.OrderAck.from_wire(ack)` or
        `FillInfo.from_wire(ack.get("fill"))`."""
        if client_oid is None:
            client_oid = f"te-{int(time.time() * 1000)}-{symbol[:6]}"
        req = _place_req(
            self._exchange,
            client_oid=client_oid,
            symbol=symbol,
            side=side,
            px=px,
            qty=qty,
            kind="limit",
            tif=tif,
        )
        resp = self._cli.send(req)
        latency_ms = resp.get("_meta", {}).get("latency_ms")
        if not is_ok(resp):
            err = resp.get("result", {}).get("Err", {})
            self._record(
                "place",
                strategy=strategy,
                label=label,
                side=side,
                symbol=symbol,
                px=px,
                qty=qty,
                client_oid=client_oid,
                ok=False,
                latency_ms=latency_ms,
                error=str(err),
            )
            raise RuntimeError(f"place failed: {err}")
        ack = unwrap(resp)
        fill = FillInfo.from_wire(ack.get("fill"))
        _log_fill("place", label, ack)
        self._record(
            "place",
            strategy=strategy,
            label=label,
            side=side,
            symbol=symbol,
            px=px,
            qty=qty,
            client_oid=client_oid,
            exchange_oid=ack.get("exchange_oid"),
            ok=True,
            latency_ms=latency_ms,
            extra=_fill_extra(fill),
        )
        return ack

    def place_market(
        self,
        symbol: str,
        side: str,
        qty: float,
        client_oid: Optional[str] = None,
        tif: str = "IOC",
        # Attribution for the event log.
        strategy: Optional[str] = None,
        label: Optional[str] = None,
    ) -> dict:
        """Place a market order. Returns the inner OrderAck dict on success;
        raises `RuntimeError` on Err. On the Polymarket path the ack's
        `fill` sub-map reports the amounts matched synchronously — parse via
        `typings.orders.OrderAck.from_wire(ack)`.

        Semantics (Polymarket-specific, side-dependent):
          - `side="buy"`  → `qty` is **USDC notional to spend**. The venue
            walks the ask book until cumulative `qty / cutoff_price` shares
            are filled. Required because the venue rejects buys whose
            implied USDC has >2 decimals — `Amount::shares` on a buy
            triggers this; `Amount::usdc` does not.
          - `side="sell"` → `qty` is **share count to sell**. The venue
            walks the bid book until `qty` shares are sold. Sells in USDC
            are not allowed by Polymarket.
          - `tif` must be `"IOC"` (FAK) or `"FOK"`. GTC/GTD are rejected
            by the executor because they don't make sense for a market
            order.
          - `px` is unused by the exchange but the protocol still carries
            it; we send 0.0 so the audit row remains well-formed.
        """
        tif_u = tif.upper()
        if tif_u not in {"IOC", "FOK"}:
            raise ValueError(
                f"place_market: tif must be 'IOC' or 'FOK', got {tif!r}"
            )
        if client_oid is None:
            client_oid = f"te-mkt-{int(time.time() * 1000)}-{symbol[:6]}"
        req = _place_req(
            self._exchange,
            client_oid=client_oid,
            symbol=symbol,
            side=side,
            px=0.0,
            qty=qty,
            kind="market",
            tif=tif_u,
        )
        resp = self._cli.send(req)
        latency_ms = resp.get("_meta", {}).get("latency_ms")
        if not is_ok(resp):
            err = resp.get("result", {}).get("Err", {})
            self._record(
                "place_market",
                strategy=strategy,
                label=label,
                side=side,
                symbol=symbol,
                qty=qty,
                client_oid=client_oid,
                ok=False,
                latency_ms=latency_ms,
                error=str(err),
                extra={"tif": tif_u},
            )
            raise RuntimeError(f"place_market failed: {err}")
        ack = unwrap(resp)
        fill = FillInfo.from_wire(ack.get("fill"))
        _log_fill("place_market", label, ack)
        self._record(
            "place_market",
            strategy=strategy,
            label=label,
            side=side,
            symbol=symbol,
            qty=qty,
            client_oid=client_oid,
            exchange_oid=ack.get("exchange_oid"),
            ok=True,
            latency_ms=latency_ms,
            extra={"tif": tif_u, **_fill_extra(fill)},
        )
        return ack

    def cancel(
        self,
        exchange_oid: str,
        *,
        strategy: Optional[str] = None,
        label: Optional[str] = None,
    ) -> dict:
        resp = self._cli.send(_cancel_req(self._exchange, exchange_oid))
        latency_ms = resp.get("_meta", {}).get("latency_ms")
        if not is_ok(resp):
            err = resp.get("result", {}).get("Err", {})
            # NotFound is a soft outcome — the order already terminated.
            if isinstance(err, dict) and err.get("kind") == "not_found":
                log.debug(f"cancel {exchange_oid}: already gone")
                self._record(
                    "cancel",
                    strategy=strategy,
                    label=label,
                    exchange_oid=exchange_oid,
                    ok=True,
                    latency_ms=latency_ms,
                    extra={"count": 0, "already_gone": True},
                )
                return {"result": "cancel_count", "count": 0}
            self._record(
                "cancel",
                strategy=strategy,
                label=label,
                exchange_oid=exchange_oid,
                ok=False,
                latency_ms=latency_ms,
                error=str(err),
            )
            raise RuntimeError(f"cancel failed: {err}")
        out = unwrap(resp)
        self._record(
            "cancel",
            strategy=strategy,
            label=label,
            exchange_oid=exchange_oid,
            ok=True,
            latency_ms=latency_ms,
            extra={"count": int(out.get("count", 0))},
        )
        return out

    def cancel_all(self, *, strategy: Optional[str] = None) -> int:
        resp = self._cli.send(_cancel_all_req(self._exchange))
        latency_ms = resp.get("_meta", {}).get("latency_ms")
        if not is_ok(resp):
            err = resp.get("result", {}).get("Err", {})
            self._record(
                "cancel_all",
                strategy=strategy,
                ok=False,
                latency_ms=latency_ms,
                error=str(err),
            )
            raise RuntimeError(f"cancel_all failed: {err}")
        inner = unwrap(resp)
        count = int(inner.get("count", 0))
        self._record(
            "cancel_all",
            strategy=strategy,
            ok=True,
            latency_ms=latency_ms,
            extra={"count": count},
        )
        return count

    def heartbeat(self) -> dict:
        """Send one Heartbeat. Returns the raw response dict (with `_meta.latency_ms`)
        so callers can decide how to handle Err / req_id mismatch."""
        req = _heartbeat_req(self._exchange)
        resp = self._cli.send(req)
        # Attach the original req_id so the CLI / caller can sanity-check
        # the round-trip identity without re-parsing the request.
        resp.setdefault("_meta", {})["req_id"] = req["req_id"]
        self._record(
            "heartbeat",
            ok=is_ok(resp),
            latency_ms=resp.get("_meta", {}).get("latency_ms"),
        )
        return resp

    # ── inner ──

    def _record(self, kind: str, **fields) -> None:
        if self._event_log is None:
            return
        self._event_log.record_action(kind=kind, exchange=self._exchange, **fields)


# ── CLI entrypoint (replaces scripts/poly_heartbeat.py) ─────────────────────


def main(argv: Optional[list[str]] = None) -> int:
    """Phase 1 smoke test — send N Heartbeats to the executor and print
    each round-trip. No money at risk."""
    import argparse
    import json
    import sys

    p = argparse.ArgumentParser(
        prog="python -m scripts.trading_engine.io.order_router",
        description="Heartbeat smoke test — round-trip the executor ROUTER N times.",
    )
    p.add_argument("--endpoint", default="tcp://127.0.0.1:5557")
    p.add_argument("--exchange", default="polymarket")
    p.add_argument("--count", type=int, default=3)
    p.add_argument("--timeout-ms", type=int, default=5_000)
    args = p.parse_args(argv)

    print(f"connecting DEALER → {args.endpoint}")
    all_ok = True
    with OrderRouter(
        args.endpoint, exchange=args.exchange, timeout_ms=args.timeout_ms
    ) as r:
        for i in range(args.count):
            resp = r.heartbeat()
            print(f"[{i + 1}/{args.count}] ← {json.dumps(resp, default=str)}")
            if not is_ok(resp):
                all_ok = False
                print(f"  ! result is Err: {resp.get('result')}", file=sys.stderr)
                continue
            sent_id = resp.get("_meta", {}).get("req_id")
            got_id = resp.get("req_id")
            if sent_id != got_id:
                all_ok = False
                print(
                    f"  ! req_id mismatch: sent {sent_id}, got {got_id}",
                    file=sys.stderr,
                )

    print()
    print("RESULT:", "ok" if all_ok else "FAIL")
    return 0 if all_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
