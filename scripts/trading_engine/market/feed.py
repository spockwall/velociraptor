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


@dataclasses.dataclass
class Trade:
    """One public last-trade event (zmq_server `LastTradeTopic`).

    Mirrors `libs::protocol::LastTradePrice`. `exchange` / `symbol` are taken
    from the ZMQ topic, not the payload, so they match the keys used for
    quotes (the payload's enum `exchange` serialises differently for
    binance_spot)."""

    exchange: str
    symbol: str
    price: float
    size: float
    side: str               # taker side: "BUY" | "SELL"
    timestamp: str          # RFC3339 from the payload (as-is)
    trade_id: Optional[int]
    received_ns: int        # local wall clock at receive


# ZMQ topic suffix the snapshot publisher appends for last-trade events
# (`topics/trade.rs`: "{exchange}:{symbol}:last_trade").
_TRADE_SUFFIX = ":last_trade"


def _topic_key(exchange: str, symbol: str) -> str:
    return f"{exchange}:{symbol}"


class MarketFeed:
    """Background SUB that keeps `{(exchange, symbol) -> latest Quote}`."""

    def __init__(
        self,
        pub_endpoint: str = "tcp://127.0.0.1:5555",
        router_endpoint: str = "tcp://127.0.0.1:5556",
        sub_type: str = "snapshot",   # "snapshot" or "bba"
        on_quote=None,
        on_trade=None,
    ):
        self._pub = pub_endpoint
        self._router = router_endpoint
        self._sub_type = sub_type
        # Optional push callbacks invoked AFTER the latest value is stored,
        # on the feed thread. The engine sets these to enqueue events on the
        # dispatcher; they must be cheap and non-blocking (just queue.put).
        # The dict store is always kept regardless, so `latest()` /
        # `latest_trade()` (observer, momentum) still work unchanged.
        self._on_quote = on_quote
        self._on_trade = on_trade
        self._ctx = zmq.Context.instance()
        self._sock: Optional[zmq.Socket] = None
        self._dealer: Optional[zmq.Socket] = None
        self._dealer_lock = threading.Lock()
        self._quotes: dict[str, Quote] = {}
        # Latest last-trade per (exchange, symbol), same key scheme as quotes.
        self._trades: dict[str, Trade] = {}
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._subscribed: set[str] = set()
        # Trade topics carry a ":last_trade" suffix and need no DEALER
        # handshake (server publishes them unconditionally — see trade.rs),
        # so they're tracked separately from `_subscribed`.
        self._subscribed_trades: set[str] = set()

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

    def subscribe_trades(
        self, symbols: Iterable[str], exchange: str = "polymarket"
    ) -> None:
        """Subscribe to `{exchange}:{symbol}:last_trade` topics.

        Unlike snapshots, the server publishes last-trade events without a
        registry check (`trade.rs`), so no DEALER handshake is needed — only
        the SUB-side prefix filter."""
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = _topic_key(exchange, sym)
            if key in self._subscribed_trades:
                continue
            self._sock.setsockopt(
                zmq.SUBSCRIBE, (key + _TRADE_SUFFIX).encode()
            )
            self._subscribed_trades.add(key)
            log.debug("subscribed %s%s", key, _TRADE_SUFFIX)

    def unsubscribe_trades(
        self, symbols: Iterable[str], exchange: str = "polymarket"
    ) -> None:
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = _topic_key(exchange, sym)
            if key not in self._subscribed_trades:
                continue
            self._sock.setsockopt(
                zmq.UNSUBSCRIBE, (key + _TRADE_SUFFIX).encode()
            )
            self._subscribed_trades.discard(key)
            with self._lock:
                self._trades.pop(key, None)
            log.debug("unsubscribed %s%s", key, _TRADE_SUFFIX)

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

    def latest_trade(
        self, symbol: str, exchange: str = "polymarket"
    ) -> Optional[Trade]:
        """Most recent last-trade for `(exchange, symbol)`, or None if none
        seen yet. Requires a prior `subscribe_trades([symbol], exchange)`."""
        with self._lock:
            return self._trades.get(_topic_key(exchange, symbol))

    def trades_all(self) -> list[Trade]:
        """Copy of the latest Trade per (exchange, symbol)."""
        with self._lock:
            return list(self._trades.values())

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
                decoded = msgpack.unpackb(payload, raw=False)
            except Exception as e:  # noqa: BLE001
                log.warning("market feed decode: %s", e)
                continue
            topic_str = topic.decode()

            # Last-trade topics carry a ":last_trade" suffix; strip it first
            # so the remaining "{exchange}:{symbol}" parses the same way as a
            # snapshot topic (Polymarket token ids contain no ':').
            if topic_str.endswith(_TRADE_SUFFIX):
                base = topic_str[: -len(_TRADE_SUFFIX)]
                try:
                    exchange, symbol = base.split(":", 1)
                except ValueError:
                    continue
                self._handle_trade(exchange, symbol, decoded, time.time_ns())
                continue

            snap = decoded
            # topic = b"{exchange}:{symbol}"
            try:
                exchange, symbol = topic_str.split(":", 1)
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
            if self._on_quote is not None:
                self._on_quote(exchange, symbol, q)

    def _handle_trade(
        self, exchange: str, symbol: str, msg: object, received_ns: int
    ) -> None:
        """Decode a `LastTradePrice` map into a Trade and store the latest
        one per (exchange, symbol). `exchange`/`symbol` come from the topic
        (the payload enum serialises binance_spot differently)."""
        if not isinstance(msg, dict):
            return
        try:
            price = float(msg.get("price"))
            size = float(msg.get("size"))
        except (TypeError, ValueError):
            log.warning("trade decode: bad price/size in %s:%s", exchange, symbol)
            return
        tid = msg.get("trade_id")
        t = Trade(
            exchange=exchange,
            symbol=symbol,
            price=price,
            size=size,
            side=str(msg.get("side", "")),
            timestamp=str(msg.get("timestamp", "")),
            trade_id=int(tid) if isinstance(tid, (int, float)) else None,
            received_ns=received_ns,
        )
        with self._lock:
            self._trades[_topic_key(exchange, symbol)] = t
        if self._on_trade is not None:
            self._on_trade(exchange, symbol, t)
