"""SUB on the user PUB. Surfaces fills + order_updates to a callback.

Topics: `user.polymarket.{fill,order_update}`.

This module also doubles as the listener CLI when run as a module:

    python -m scripts.trading_engine.io.user_feed
    python -m scripts.trading_engine.io.user_feed --topic-prefix user.polymarket.fill

That replaces the standalone `scripts/poly_user_listener.py` script.
"""

from __future__ import annotations

import logging
import threading
import time
from typing import Any, Callable, Optional

import msgpack
import zmq

log = logging.getLogger(__name__)

EventHandler = Callable[[str, dict], None]

DEFAULT_ENDPOINT = "tcp://127.0.0.1:5559"
DEFAULT_TOPIC_PREFIX = "user."


class UserFeed:
    def __init__(
        self,
        endpoint: str = DEFAULT_ENDPOINT,
        on_event: Optional[EventHandler] = None,
    ):
        self._endpoint = endpoint
        self._on_event = on_event or (lambda topic, ev: None)
        self._ctx = zmq.Context.instance()
        self._sock: Optional[zmq.Socket] = None
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None

    def start(self, topic_prefix: str = "user.polymarket.") -> None:
        if self._thread is not None:
            return
        self._sock = self._ctx.socket(zmq.SUB)
        self._sock.setsockopt(zmq.LINGER, 0)
        self._sock.setsockopt(zmq.RCVTIMEO, 250)
        self._sock.setsockopt(zmq.SUBSCRIBE, topic_prefix.encode())
        self._sock.connect(self._endpoint)
        self._thread = threading.Thread(
            target=self._loop, name="user-feed", daemon=True
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2)
        if self._sock is not None:
            self._sock.close(0)
            self._sock = None

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                frames = self._sock.recv_multipart()  # type: ignore[union-attr]
            except zmq.error.Again:
                continue
            except zmq.error.ZMQError as e:
                log.warning(f"user feed recv error: {e}")
                continue
            if not frames:
                continue
            topic = frames[0].decode("utf-8", errors="replace")
            payload = frames[-1] if len(frames) >= 2 else b""
            try:
                ev = msgpack.unpackb(payload, raw=False)
            except Exception as e:  # noqa: BLE001
                log.warning(f"user feed decode: {e}")
                continue
            if not isinstance(ev, dict):
                continue
            try:
                self._on_event(topic, ev)
            except Exception:  # noqa: BLE001
                log.exception("user-event handler raised")


# ── CLI entrypoint (replaces scripts/poly_user_listener.py) ─────────────────


def _fmt_event(topic: str, ev: dict[str, Any]) -> str:
    """Pretty-print one user event in a single line."""
    import json

    kind = ev.get("type", "?")
    ts_ns = ev.get("ts_ns", 0)
    ts = time.strftime("%H:%M:%S", time.localtime(ts_ns / 1e9)) if ts_ns else "????????"
    if kind == "fill":
        return (
            f"[{ts}] {topic}  side={ev.get('side')}  "
            f"px={ev.get('px')}  qty={ev.get('qty')}  "
            f"trade={ev.get('trade_id')}  taker={ev.get('taker_oid')}  "
            f"client={ev.get('client_oid')}"
        )
    if kind == "order_update":
        return (
            f"[{ts}] {topic}  status={ev.get('status')}  side={ev.get('side')}  "
            f"px={ev.get('px')}  qty={ev.get('qty')}  filled={ev.get('filled')}  "
            f"oid={ev.get('exchange_oid')}"
        )
    return f"[{ts}] {topic}  {json.dumps(ev, default=str)}"


def main(argv: Optional[list[str]] = None) -> int:
    import argparse
    import signal
    import sys

    p = argparse.ArgumentParser(
        prog="python -m scripts.trading_engine.io.user_feed",
        description="Subscribe to the user PUB and print events to stdout.",
    )
    p.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    p.add_argument(
        "--topic-prefix",
        default=DEFAULT_TOPIC_PREFIX,
        help=(
            "ZMQ SUB filter prefix. 'user.' matches all user events; "
            "'user.polymarket.fill' matches Polymarket fills only."
        ),
    )
    args = p.parse_args(argv)

    print(
        f"connecting SUB → {args.endpoint}  (filter='{args.topic_prefix}')",
        file=sys.stderr,
    )

    def _print(topic: str, ev: dict) -> None:
        print(_fmt_event(topic, ev), flush=True)

    feed = UserFeed(args.endpoint, on_event=_print)
    feed.start(topic_prefix=args.topic_prefix)

    stop = threading.Event()
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    try:
        stop.wait()
    finally:
        feed.stop()
        print("\nbye", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
