"""SUB on the market PUB. Maintains the latest BBA per (exchange, symbol).

Topic format from the zmq_server snapshot publisher is plain
`{exchange}:{symbol}` (see `zmq_server/src/topics/snapshot.rs`). This module
subscribes per topic and decodes the msgpack `OrderbookSnapshot` into a
small `Quote`. The strategy only needs `best_bid`, `best_ask`, and `mid`.

Two sockets:
  - SUB on `--market-pub` (default tcp://127.0.0.1:5555): receives frames.
  - DEALER on `--market-router` (default tcp://127.0.0.1:5556): handshakes
    with the zmq_server registry. The server's `dispatch()` only forwards
    snapshots to topics that have a registered subscription, so the
    handshake is mandatory.

Quotes are keyed by the full `"{exchange}:{symbol}"` topic string so we
don't collide across exchanges (e.g. Binance's `btcusdt` vs. Kalshi's
`BTCUSDT` — and Polymarket token ids are exchange-unique anyway).
"""

from __future__ import annotations

import dataclasses
import logging
import threading
from typing import Iterable, Optional

import msgpack
import zmq

log = logging.getLogger(__name__)


@dataclasses.dataclass
class Quote:
    exchange: str
    symbol: str
    best_bid: Optional[float]
    best_ask: Optional[float]
    mid: Optional[float]
    sequence: int
    received_ns: int   # local wall clock at receive

    @property
    def is_two_sided(self) -> bool:
        return self.best_bid is not None and self.best_ask is not None


def _topic_key(exchange: str, symbol: str) -> str:
    return f"{exchange}:{symbol}"


class MarketFeed:
    """Background SUB that keeps `{(exchange, symbol) -> latest Quote}`."""

    def __init__(
        self,
        pub_endpoint: str = "tcp://127.0.0.1:5555",
        router_endpoint: str = "tcp://127.0.0.1:5556",
        sub_type: str = "snapshot",   # "snapshot" or "bba"
    ):
        self._pub = pub_endpoint
        self._router = router_endpoint
        self._sub_type = sub_type
        self._ctx = zmq.Context.instance()
        self._sock: Optional[zmq.Socket] = None
        self._dealer: Optional[zmq.Socket] = None
        self._dealer_lock = threading.Lock()
        self._quotes: dict[str, Quote] = {}
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._subscribed: set[str] = set()

    # ── public ──

    def start(self) -> None:
        if self._thread is not None:
            return
        # SUB — actual data path.
        self._sock = self._ctx.socket(zmq.SUB)
        self._sock.setsockopt(zmq.LINGER, 0)
        self._sock.setsockopt(zmq.RCVTIMEO, 250)   # ms; lets us check stop flag
        self._sock.connect(self._pub)
        # DEALER — control plane. Without this handshake the server's
        # registry stays empty and no snapshots are sent to our SUB.
        self._dealer = self._ctx.socket(zmq.DEALER)
        self._dealer.setsockopt(zmq.LINGER, 0)
        self._dealer.setsockopt(zmq.SNDTIMEO, 1_000)
        self._dealer.setsockopt(zmq.RCVTIMEO, 1_000)
        self._dealer.connect(self._router)
        self._thread = threading.Thread(
            target=self._loop, name="market-feed", daemon=True
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2)
        if self._sock is not None:
            self._sock.close(0)
            self._sock = None
        if self._dealer is not None:
            self._dealer.close(0)
            self._dealer = None

    def subscribe(self, symbols: Iterable[str], exchange: str = "polymarket") -> None:
        """Subscribe to `polymarket:{asset}` topics by default (back-compat).
        Pass `exchange=` to subscribe other markets (e.g. "binance", "kalshi")."""
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = _topic_key(exchange, sym)
            if key in self._subscribed:
                continue
            self._sock.setsockopt(zmq.SUBSCRIBE, key.encode())
            self._handshake("subscribe", exchange, sym)
            self._subscribed.add(key)
            log.debug("subscribed %s", key)

    def unsubscribe(self, symbols: Iterable[str], exchange: str = "polymarket") -> None:
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = _topic_key(exchange, sym)
            if key not in self._subscribed:
                continue
            self._sock.setsockopt(zmq.UNSUBSCRIBE, key.encode())
            self._handshake("unsubscribe", exchange, sym)
            self._subscribed.discard(key)
            with self._lock:
                self._quotes.pop(key, None)
            log.debug("unsubscribed %s", key)

    # ── control-plane handshake ──

    def _handshake(self, action: str, exchange: str, symbol: str) -> None:
        """Send `{action, exchange, symbol, type}` over DEALER and read the
        ACK. Logs but does not raise on failure — the SUB filter is still
        applied so we'll start receiving as soon as the server registers us."""
        if self._dealer is None:
            return
        req = {
            "action": action,
            "exchange": exchange,
            "symbol": symbol,
            "type": self._sub_type,
        }
        with self._dealer_lock:
            try:
                self._dealer.send_json(req)
                ack = self._dealer.recv_json()
            except zmq.error.ZMQError as e:
                log.warning("handshake %s %s:%s failed: %s", action, exchange, symbol, e)
                return
        status = ack.get("status") if isinstance(ack, dict) else None
        if status != "ok":
            log.warning(
                "handshake %s %s:%s rejected: %s", action, exchange, symbol, ack
            )

    def latest(self, symbol: str, exchange: str = "polymarket") -> Optional[Quote]:
        """Latest quote for `(exchange, symbol)`. Defaults to polymarket for
        back-compat with the existing strategy."""
        with self._lock:
            return self._quotes.get(_topic_key(exchange, symbol))

    def snapshot_all(self) -> list[Quote]:
        """Copy of every Quote currently held. Used by step 0 reporting."""
        with self._lock:
            return list(self._quotes.values())

    # ── inner ──

    def _loop(self) -> None:
        import time

        while not self._stop.is_set():
            try:
                frames = self._sock.recv_multipart()  # type: ignore[union-attr]
            except zmq.error.Again:
                continue
            except zmq.error.ZMQError as e:
                log.warning("market feed recv error: %s", e)
                continue
            if len(frames) != 2:
                continue
            topic, payload = frames
            try:
                snap = msgpack.unpackb(payload, raw=False)
            except Exception as e:  # noqa: BLE001
                log.warning("market feed decode: %s", e)
                continue
            # topic = b"{exchange}:{symbol}"
            try:
                exchange, symbol = topic.decode().split(":", 1)
            except ValueError:
                continue

            bb = snap.get("best_bid")
            ba = snap.get("best_ask")
            bid_px = bb[0] if isinstance(bb, (list, tuple)) and len(bb) >= 1 else None
            ask_px = ba[0] if isinstance(ba, (list, tuple)) and len(ba) >= 1 else None
            mid = snap.get("mid")
            if mid is None and bid_px is not None and ask_px is not None:
                mid = (bid_px + ask_px) / 2.0

            q = Quote(
                exchange=exchange,
                symbol=symbol,
                best_bid=bid_px,
                best_ask=ask_px,
                mid=mid,
                sequence=int(snap.get("sequence", 0)),
                received_ns=time.time_ns(),
            )
            with self._lock:
                self._quotes[_topic_key(exchange, symbol)] = q
