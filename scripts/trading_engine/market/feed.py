"""ZMQ SUB on the zmq_server market PUB.

`MarketFeed` is **transport only**. It owns no cache. Each incoming
frame is decoded and handed to a producer callback supplied by the
`Engine`; the callback's job is to enqueue an `Event` onto the
engine's queue. The `Dispatcher` then updates `MarketState` and fans
out to strategy callbacks.

Topic format from the zmq_server snapshot publisher is plain
`{exchange}:{symbol}` (see `zmq_server/src/topics/snapshot.rs`).
Last-trade topics carry a `:last_trade` suffix
(`zmq_server/src/topics/trade.rs`).

Two sockets:

  - SUB on `--market-pub` (default tcp://127.0.0.1:5555): receives frames.
  - DEALER on `--market-router` (default tcp://127.0.0.1:5556): handshakes
    with the zmq_server registry. The server's `dispatch()` only forwards
    snapshots to topics that have a registered subscription, so the
    handshake is mandatory for static (non-rolling) topics. Rolling
    topics (`polymarket:{base_slug}` / `kalshi:{series}`) are published
    unconditionally — pass `handshake=False` for those.

Rollover detection: rolling-topic snapshots carry a `full_slug` field.
The feed tracks the last seen `full_slug` per (exchange, topic_id) and
emits a `RolloverEvent` whenever it changes (including the first frame).

Symbol semantics:

  - For static exchanges, `{exchange}:{symbol}` IS the topic, and the
    payload's `symbol` matches the topic.
  - For rolling topics, the topic id is the **base_slug** (Polymarket)
    or **series** (Kalshi). The payload's `symbol` is the per-window
    asset_id / ticker. We deliver TWO QuoteEvents on each frame — one
    keyed by the topic id (so strategies that subscribed by base_slug
    keep working through rollover without resubscribing) and one keyed
    by the asset_id (so strategies that read `market.quote(ex, asset_id)`
    after seeing a RolloverEvent get the latest value).
"""

from __future__ import annotations

import dataclasses
import logging
import threading
import time
from typing import Callable, Iterable, Optional

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
    received_ns: int  # local wall clock at receive
    # For rolling markets (Polymarket/Kalshi) the publisher stamps the
    # current window's full_slug into the payload — `None` for static
    # exchanges (binance/okx/etc.) and for older feeds without the field.
    full_slug: Optional[str] = None

    @property
    def is_two_sided(self) -> bool:
        return self.best_bid is not None and self.best_ask is not None


@dataclasses.dataclass
class Trade:
    """One public last-trade event (zmq_server `LastTradeTopic`).

    Mirrors `libs::protocol::LastTradePrice`. `exchange` / `symbol` are
    taken from the ZMQ topic, not the payload, so they match the keys
    used for quotes (the payload's enum `exchange` serialises
    differently for binance_spot)."""

    exchange: str
    symbol: str
    price: float
    size: float
    side: str  # taker side: "BUY" | "SELL"
    timestamp: str  # RFC3339 from the payload (as-is)
    trade_id: Optional[int]
    received_ns: int  # local wall clock at receive
    # See Quote.full_slug.
    full_slug: Optional[str] = None


# Producer-callback signatures used by Engine to enqueue events.
QuoteHandler = Callable[[str, str, Quote], None]
TradeHandler = Callable[[str, str, Trade], None]
RolloverHandler = Callable[[str, str, str, str], None]
# Args: (exchange, base_slug, new_full_slug, asset_id)


# ZMQ topic suffix the snapshot publisher appends for last-trade events
# (`topics/trade.rs`: "{exchange}:{symbol}:last_trade").
_TRADE_SUFFIX = ":last_trade"


class MarketFeed:
    """Background SUB thread. Stateless wrt market data — every frame
    becomes a producer callback invocation."""

    def __init__(
        self,
        pub_endpoint: str = "tcp://127.0.0.1:5555",
        router_endpoint: str = "tcp://127.0.0.1:5556",
        sub_type: str = "snapshot",  # "snapshot" or "bba"
        on_quote: Optional[QuoteHandler] = None,
        on_trade: Optional[TradeHandler] = None,
        on_rollover: Optional[RolloverHandler] = None,
    ):
        self._pub = pub_endpoint
        self._router = router_endpoint
        self._sub_type = sub_type
        # Producer callbacks. Invoked on the SUB thread; the Engine wires
        # them to enqueue (QuoteEvent / TradeEvent / RolloverEvent) onto
        # the dispatcher's queue. None = drop silently.
        self._on_quote = on_quote
        self._on_trade = on_trade
        self._on_rollover = on_rollover
        # Last-seen full_slug per (exchange, base_slug) — drives rollover
        # detection. None = no frame yet.
        self._last_full_slug: dict[tuple[str, str], Optional[str]] = {}
        self._ctx = zmq.Context.instance()
        self._sock: Optional[zmq.Socket] = None
        self._dealer: Optional[zmq.Socket] = None
        self._dealer_lock = threading.Lock()
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._subscribed: set[tuple[str, str]] = set()
        # Trade topics carry a ":last_trade" suffix and need no DEALER
        # handshake (server publishes them unconditionally — see trade.rs),
        # so they're tracked separately from `_subscribed`.
        self._subscribed_trades: set[tuple[str, str]] = set()

    # ── public ──

    def start(self) -> None:
        if self._thread is not None:
            return
        # SUB — actual data path.
        self._sock = self._ctx.socket(zmq.SUB)
        self._sock.setsockopt(zmq.LINGER, 0)
        self._sock.setsockopt(zmq.RCVTIMEO, 250)  # ms; lets us check stop flag
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

    def subscribe(
        self,
        symbols: Iterable[str],
        exchange: str = "polymarket",
        *,
        handshake: bool = True,
    ) -> None:
        """Subscribe to `{exchange}:{symbol}` snapshot topics.

        `handshake=True` (the default) sends the DEALER control-plane
        subscribe so the server's registry forwards us frames. Static
        exchanges (binance/binance_spot/...) need this.

        For **rolling** topics (`polymarket:{base_slug}` /
        `kalshi:{series}`), the server publishes unconditionally —
        bypass the registry by passing `handshake=False`. The SUB-side
        prefix filter alone is sufficient.
        """
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = (exchange, sym)
            if key in self._subscribed:
                continue
            topic = f"{exchange}:{sym}"
            self._sock.setsockopt(zmq.SUBSCRIBE, topic.encode())
            if handshake:
                self._handshake("subscribe", exchange, sym)
            self._subscribed.add(key)
            log.debug(f"subscribed {topic} (handshake={handshake})")

    def unsubscribe(
        self,
        symbols: Iterable[str],
        exchange: str = "polymarket",
        *,
        handshake: bool = True,
    ) -> None:
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = (exchange, sym)
            if key not in self._subscribed:
                continue
            topic = f"{exchange}:{sym}"
            self._sock.setsockopt(zmq.UNSUBSCRIBE, topic.encode())
            if handshake:
                self._handshake("unsubscribe", exchange, sym)
            self._subscribed.discard(key)
            log.debug(f"unsubscribed {topic} (handshake={handshake})")

    def subscribe_trades(
        self, symbols: Iterable[str], exchange: str = "polymarket"
    ) -> None:
        """Subscribe to `{exchange}:{symbol}:last_trade` topics.

        Unlike snapshots, the server publishes last-trade events without
        a registry check (`trade.rs`), so no DEALER handshake is needed
        — only the SUB-side prefix filter."""
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = (exchange, sym)
            if key in self._subscribed_trades:
                continue
            topic = f"{exchange}:{sym}{_TRADE_SUFFIX}"
            self._sock.setsockopt(zmq.SUBSCRIBE, topic.encode())
            self._subscribed_trades.add(key)
            log.debug(f"subscribed {topic}")

    def unsubscribe_trades(
        self, symbols: Iterable[str], exchange: str = "polymarket"
    ) -> None:
        assert self._sock is not None, "start() the feed first"
        for sym in symbols:
            key = (exchange, sym)
            if key not in self._subscribed_trades:
                continue
            topic = f"{exchange}:{sym}{_TRADE_SUFFIX}"
            self._sock.setsockopt(zmq.UNSUBSCRIBE, topic.encode())
            self._subscribed_trades.discard(key)
            log.debug(f"unsubscribed {topic}")

    # ── control-plane handshake ──

    def _handshake(self, action: str, exchange: str, symbol: str) -> None:
        """Send `{action, exchange, symbol, type}` over DEALER and read
        the ACK. Logs but does not raise on failure — the SUB filter is
        still applied so we'll start receiving as soon as the server
        registers us."""
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
                log.warning(f"handshake {action} {exchange}:{symbol} failed: {e}")
                return
        status = ack.get("status") if isinstance(ack, dict) else None
        if status != "ok":
            log.warning(f"handshake {action} {exchange}:{symbol} rejected: {ack}")

    # ── inner ──

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                frames = self._sock.recv_multipart()  # type: ignore[union-attr]
            except zmq.error.Again:
                continue
            except zmq.error.ZMQError as e:
                log.warning(f"market feed recv error: {e}")
                continue
            if len(frames) != 2:
                continue
            topic, payload = frames
            try:
                decoded = msgpack.unpackb(payload, raw=False)
            except Exception as e:  # noqa: BLE001
                log.warning(f"market feed decode: {e}")
                continue
            topic_str = topic.decode()

            # Last-trade topics carry a ":last_trade" suffix; strip it
            # first so the remaining "{exchange}:{topic_id}" parses the
            # same way as a snapshot topic. `topic_id` is the asset_id
            # for static exchanges and the BASE_SLUG for rolling
            # Polymarket/Kalshi.
            if topic_str.endswith(_TRADE_SUFFIX):
                base = topic_str[: -len(_TRADE_SUFFIX)]
                try:
                    exchange, topic_id = base.split(":", 1)
                except ValueError:
                    continue
                self._handle_trade(exchange, topic_id, decoded, time.time_ns())
                continue

            try:
                exchange, topic_id = topic_str.split(":", 1)
            except ValueError:
                continue
            self._handle_snapshot(exchange, topic_id, decoded, time.time_ns())

    def _handle_snapshot(
        self, exchange: str, topic_id: str, snap: object, received_ns: int
    ) -> None:
        if not isinstance(snap, dict):
            return
        bb = snap.get("best_bid")
        ba = snap.get("best_ask")
        bid_px = bb[0] if isinstance(bb, (list, tuple)) and len(bb) >= 1 else None
        ask_px = ba[0] if isinstance(ba, (list, tuple)) and len(ba) >= 1 else None
        mid = snap.get("mid")
        if mid is None and bid_px is not None and ask_px is not None:
            mid = (bid_px + ask_px) / 2.0

        full_slug = snap.get("full_slug")
        # For rolling topics the payload's `symbol` carries the real
        # per-window asset_id, which differs from the topic_id (= the
        # stable base_slug). Use it as the Quote.symbol; otherwise fall
        # back to topic_id.
        payload_symbol = snap.get("symbol") or topic_id
        q = Quote(
            exchange=exchange,
            symbol=payload_symbol,
            best_bid=bid_px,
            best_ask=ask_px,
            mid=mid,
            sequence=int(snap.get("sequence", 0)),
            received_ns=received_ns,
            full_slug=full_slug if isinstance(full_slug, str) else None,
        )

        # Rollover detection: fire BEFORE the quote so the strategy's
        # rollover handler can reset its window-scoped state before the
        # quote callback reacts to the new asset's data.
        if isinstance(full_slug, str):
            rk = (exchange, topic_id)
            last = self._last_full_slug.get(rk)
            if last != full_slug:
                self._last_full_slug[rk] = full_slug
                if self._on_rollover is not None:
                    try:
                        self._on_rollover(exchange, topic_id, full_slug, payload_symbol)
                    except Exception:  # noqa: BLE001
                        log.exception("on_rollover handler raised")

        if self._on_quote is None:
            return
        # Always emit keyed by topic_id (back-compat: subscribe-by-base_slug
        # callers read by base_slug; static-exchange callers read by their
        # asset_id, which IS the topic id).
        try:
            self._on_quote(exchange, topic_id, q)
        except Exception:  # noqa: BLE001
            log.exception("on_quote handler raised")
        # For rolling topics, ALSO emit keyed by the payload's asset_id
        # so callers that read `market.quote(ex, asset_id)` after a
        # rollover see the latest frame.
        if payload_symbol != topic_id:
            try:
                self._on_quote(exchange, payload_symbol, q)
            except Exception:  # noqa: BLE001
                log.exception("on_quote handler raised (asset_id key)")

    def _handle_trade(
        self, exchange: str, topic_id: str, msg: object, received_ns: int
    ) -> None:
        """Decode a `LastTradePrice` map. For rolling topics the payload
        carries the per-window asset_id in `symbol` — we emit the trade
        twice (once keyed by topic_id, once by asset_id) for the same
        reason as `_handle_snapshot`."""
        if not isinstance(msg, dict):
            return
        try:
            price = float(msg.get("price"))
            size = float(msg.get("size"))
        except (TypeError, ValueError):
            log.warning(f"trade decode: bad price/size in {exchange}:{topic_id}")
            return
        tid = msg.get("trade_id")
        full_slug = msg.get("full_slug")
        payload_symbol = msg.get("symbol") or topic_id
        t = Trade(
            exchange=exchange,
            symbol=payload_symbol,
            price=price,
            size=size,
            side=str(msg.get("side", "")),
            timestamp=str(msg.get("timestamp", "")),
            trade_id=int(tid) if isinstance(tid, (int, float)) else None,
            received_ns=received_ns,
            full_slug=full_slug if isinstance(full_slug, str) else None,
        )
        if self._on_trade is None:
            return
        try:
            self._on_trade(exchange, topic_id, t)
        except Exception:  # noqa: BLE001
            log.exception("on_trade handler raised")
        if payload_symbol != topic_id:
            try:
                self._on_trade(exchange, payload_symbol, t)
            except Exception:  # noqa: BLE001
                log.exception("on_trade handler raised (asset_id key)")
