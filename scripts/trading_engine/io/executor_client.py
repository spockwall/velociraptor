"""
Reusable ZMQ DEALER client for the Velociraptor executor's order ROUTER.

Wire format (mirrors `libs/src/protocol/orders.rs`):

    request:  msgpack-named-map of OrderRequest  { req_id, exchange, action }
    response: msgpack-named-map of OrderResponse { req_id, result }

ROUTER framing on the executor side is `[identity | empty | payload]`. A
DEALER socket auto-prepends an empty delimiter, so we send a single payload
frame and recv a single payload frame here.

Usage:

    from executor_client import ExecutorClient, heartbeat, place, cancel

    with ExecutorClient("tcp://127.0.0.1:5557") as cli:
        resp = cli.send(heartbeat("polymarket"))
        print(resp)
"""

from __future__ import annotations

import itertools
import time
from contextlib import AbstractContextManager
from typing import Any

import msgpack
import zmq

DEFAULT_ENDPOINT = "tcp://127.0.0.1:5557"
DEFAULT_TIMEOUT_MS = 5_000


# ── Request builders ──────────────────────────────────────────────────────────

_req_seq = itertools.count(1)


def _next_req_id() -> int:
    return next(_req_seq)


def heartbeat(exchange: str) -> dict[str, Any]:
    return {
        "req_id": _next_req_id(),
        "exchange": exchange,
        "action": {"action": "heartbeat"},
    }


def place(
    exchange: str,
    *,
    client_oid: str,
    symbol: str,
    side: str,
    px: float,
    qty: float,
    kind: str = "limit",
    tif: str = "GTC",
) -> dict[str, Any]:
    return {
        "req_id": _next_req_id(),
        "exchange": exchange,
        "action": {
            "action": "place",
            "client_oid": client_oid,
            "symbol": symbol,
            "side": side,
            "kind": kind,
            "px": px,
            "qty": qty,
            "tif": tif,
        },
    }


def cancel(exchange: str, exchange_oid: str) -> dict[str, Any]:
    return {
        "req_id": _next_req_id(),
        "exchange": exchange,
        "action": {"action": "cancel", "exchange_oid": exchange_oid},
    }


def cancel_all(exchange: str) -> dict[str, Any]:
    return {
        "req_id": _next_req_id(),
        "exchange": exchange,
        "action": {"action": "cancel_all"},
    }


def cancel_market(exchange: str, symbol: str) -> dict[str, Any]:
    return {
        "req_id": _next_req_id(),
        "exchange": exchange,
        "action": {"action": "cancel_market", "symbol": symbol},
    }


# ── Client ────────────────────────────────────────────────────────────────────


class ExecutorClient(AbstractContextManager["ExecutorClient"]):
    def __init__(
        self,
        endpoint: str = DEFAULT_ENDPOINT,
        *,
        timeout_ms: int = DEFAULT_TIMEOUT_MS,
    ) -> None:
        self._endpoint = endpoint
        self._timeout_ms = timeout_ms
        self._ctx = zmq.Context.instance()
        self._sock: zmq.Socket | None = None

    def __enter__(self) -> "ExecutorClient":
        self.connect()
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def connect(self) -> None:
        sock = self._ctx.socket(zmq.DEALER)
        sock.setsockopt(zmq.LINGER, 0)
        sock.setsockopt(zmq.RCVTIMEO, self._timeout_ms)
        sock.setsockopt(zmq.SNDTIMEO, self._timeout_ms)
        sock.connect(self._endpoint)
        self._sock = sock

    def close(self) -> None:
        if self._sock is not None:
            self._sock.close(0)
            self._sock = None

    def send(self, request: dict[str, Any]) -> dict[str, Any]:
        """Send one request, block for the matching response. Raises on timeout."""
        if self._sock is None:
            raise RuntimeError("ExecutorClient: not connected")
        payload = msgpack.packb(request, use_bin_type=True)
        # DEALER auto-prepends the empty delimiter; ROUTER on the other side
        # strips identity + delim and treats the last frame as payload.
        self._sock.send(payload)
        t0 = time.monotonic()
        parts = self._sock.recv_multipart()
        elapsed_ms = (time.monotonic() - t0) * 1000.0
        if not parts:
            raise RuntimeError("ExecutorClient: empty response")
        resp = msgpack.unpackb(parts[-1], raw=False)
        if isinstance(resp, dict):
            resp.setdefault("_meta", {})["latency_ms"] = round(elapsed_ms, 3)
        return resp


# ── Result helpers ────────────────────────────────────────────────────────────


def is_ok(resp: dict[str, Any]) -> bool:
    """OrderResponse.result is `Result<OrderResult, OrderError>` — rmp-serde
    encodes Result as a single-key map with `Ok` or `Err`."""
    r = resp.get("result")
    return isinstance(r, dict) and "Ok" in r


def unwrap(resp: dict[str, Any]) -> dict[str, Any]:
    r = resp.get("result")
    if isinstance(r, dict) and "Ok" in r:
        return r["Ok"]
    raise RuntimeError(f"executor returned error: {r}")


def unwrap_err(resp: dict[str, Any]) -> dict[str, Any]:
    r = resp.get("result")
    if isinstance(r, dict) and "Err" in r:
        return r["Err"]
    raise RuntimeError(f"expected Err, got {r}")
