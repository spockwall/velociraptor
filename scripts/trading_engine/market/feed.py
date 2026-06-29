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
    """Best-bid / best-ask snapshot only. Mirrors the `bba:*` ZMQ
    topic payload shape — no depth. Strategies that need the ladder
    should register an `on_snapshot` callback for the full `Snapshot`.

    `symbol` is the venue asset id (Polymarket: clobTokenId; Binance:
    e.g. `btcusdt`). `full_slug` carries the window identity for
    rolling Polymarket / Kalshi topics (`None` for static).
    """

    exchange: str
    symbol: str
    best_bid: Optional[float]
    best_ask: Optional[float]
    mid: Optional[float]
    sequence: int
    received_ns: int  # local wall clock at receive (Python side)
    # Exchange-stamped time in Unix ns from the wire payload (`0` when the
    # venue sends none). Distinct from `received_ns`, which is the Python
    # SUB-thread receive time.
    ex_timestamp: int = 0
    # Rust-side receive time in Unix ns from the wire payload.
    recv_timestamp: int = 0
    full_slug: Optional[str] = None

    @property
    def is_two_sided(self) -> bool:
        return self.best_bid is not None and self.best_ask is not None


@dataclasses.dataclass
class Snapshot:
    """Full orderbook snapshot — every depth level the publisher sent.

    Mirrors `libs::protocol::OrderbookSnapshot`. Bids are sorted high
    → low, asks low → high; the publisher does the sorting so callers
    can trust the order. See [`Quote`] for the symbol / full_slug split.
    """

    exchange: str
    symbol: str
    best_bid: Optional[float]
    best_ask: Optional[float]
    mid: Optional[float]
    sequence: int
    received_ns: int
    ex_timestamp: int = 0
    recv_timestamp: int = 0
    full_slug: Optional[str] = None
    bids: list[tuple[float, float]] = dataclasses.field(default_factory=list)
    asks: list[tuple[float, float]] = dataclasses.field(default_factory=list)

    def as_quote(self) -> "Quote":
        """Cheap BBA view used by the snapshot → quote fan-out in
        `MarketFeed._handle_snapshot`."""
        return Quote(
            exchange=self.exchange,
            symbol=self.symbol,
            best_bid=self.best_bid,
            best_ask=self.best_ask,
            mid=self.mid,
            sequence=self.sequence,
            received_ns=self.received_ns,
            ex_timestamp=self.ex_timestamp,
            recv_timestamp=self.recv_timestamp,
            full_slug=self.full_slug,
        )


@dataclasses.dataclass
class Trade:
    """One public last-trade event (zmq_server `LastTradeTopic`).

    Mirrors `libs::protocol::LastTradePrice`. See [`Quote`] for the
    symbol / full_slug split."""

    exchange: str
    symbol: str
    price: float
    size: float
    side: str  # taker side: "BUY" | "SELL"
    trade_id: Optional[int]
    received_ns: int  # local wall clock at receive (Python side)
    # Exchange-stamped trade time in Unix ns from the wire payload (`0` if
    # the venue sends none).
    ex_timestamp: int = 0
    # Rust-side receive time in Unix ns from the wire payload.
    recv_timestamp: int = 0
    full_slug: Optional[str] = None


# Producer-callback signatures used by Engine to enqueue events.
QuoteHandler = Callable[[str, str, Quote], None]
TradeHandler = Callable[[str, str, Trade], None]
# Args: (exchange, base_slug, full_slug). The dispatcher resolves
# per-token asset_ids via `MarketState.on_rollover` BEFORE invoking
# strategy callbacks — strategies just read via
# `MarketState.asset_ids(full_slug)`.
RolloverHandler = Callable[[str, str, str], None]
SnapshotHandler = Callable[[str, str, "Snapshot"], None]


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
        on_snapshot: Optional[SnapshotHandler] = None,
    ):
        self._pub = pub_endpoint
        self._router = router_endpoint
        self._sub_type = sub_type
        # Producer callbacks. Invoked on the SUB thread; the Engine wires
        # them to enqueue (QuoteEvent / TradeEvent / RolloverEvent /
        # SnapshotEvent) onto the dispatcher's queue. None = drop silently.
        # `on_snapshot` carries depth; `on_quote` is BBA-only and fired
        # off the same publisher frame.
        self._on_quote = on_quote
        self._on_trade = on_trade
        self._on_rollover = on_rollover
        self._on_snapshot = on_snapshot
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

    def prime_rollover(
        self, exchange: str, base_slug: str, full_slug: str
    ) -> None:
        """Seed the internal `_last_full_slug` cache so the next
        wire frame carrying the SAME `full_slug` does NOT trigger a
        spurious rollover event. Called by the engine after it
        synthesises a `BootstrapEvent` for the current window, so
        the dispatcher doesn't run the rollover path twice for one
        window."""
        self._last_full_slug[(exchange, base_slug)] = full_slug

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

        # `symbol` is the venue asset id (asset_id for Polymarket,
        # `btcusdt` for Binance, etc.). For rolling topics the
        # publisher also stamps `full_slug` — that's the rollover
        # identity. Static exchanges have `full_slug == None`.
        payload_symbol = snap.get("symbol") or topic_id
        full_slug = snap.get("full_slug")
        bids = _coerce_levels(snap.get("bids"))
        asks = _coerce_levels(snap.get("asks"))
        s = Snapshot(
            exchange=exchange,
            symbol=payload_symbol,
            best_bid=bid_px,
            best_ask=ask_px,
            mid=mid,
            sequence=int(snap.get("sequence", 0)),
            received_ns=received_ns,
            ex_timestamp=int(snap.get("ex_timestamp", 0) or 0),
            recv_timestamp=int(snap.get("recv_timestamp", 0) or 0),
            full_slug=full_slug if isinstance(full_slug, str) else None,
            bids=bids,
            asks=asks,
        )
        # Derived BBA view — pure projection of `s`.
        q = s.as_quote()

        # Rollover detection: a `full_slug` change vs the
        # previously-seen value on `(exchange, topic_id)` IS the
        # signal. Fires BEFORE the quote/snapshot so strategies can
        # repopulate per-window state (e.g. resolve UP/DOWN asset_ids
        # via Gamma) before the callback reacts to the new book.
        if isinstance(full_slug, str):
            rk = (exchange, topic_id)
            last = self._last_full_slug.get(rk)
            if last != full_slug:
                self._last_full_slug[rk] = full_slug
                if self._on_rollover is not None:
                    try:
                        self._on_rollover(exchange, topic_id, full_slug)
                    except Exception:  # noqa: BLE001
                        log.exception("on_rollover handler raised")

        # Single delivery keyed by `topic_id` (= base_slug for rolling,
        # asset_id for static). Strategies that need the per-token
        # asset_id look it up via `MarketState.asset_ids()`.
        if self._on_snapshot is not None:
            try:
                self._on_snapshot(exchange, topic_id, s)
            except Exception:  # noqa: BLE001
                log.exception("on_snapshot handler raised")
        if self._on_quote is not None:
            try:
                self._on_quote(exchange, topic_id, q)
            except Exception:  # noqa: BLE001
                log.exception("on_quote handler raised")

    def _handle_trade(
        self, exchange: str, topic_id: str, msg: object, received_ns: int
    ) -> None:
        """Decode a `LastTradePrice` map. See `_handle_snapshot` for
        the symbol / full_slug semantics."""
        if not isinstance(msg, dict):
            return
        try:
            price = float(msg.get("price"))
            size = float(msg.get("size"))
        except (TypeError, ValueError):
            log.warning(f"trade decode: bad price/size in {exchange}:{topic_id}")
            return
        tid = msg.get("trade_id")
        payload_symbol = msg.get("symbol") or topic_id
        full_slug = msg.get("full_slug")
        t = Trade(
            exchange=exchange,
            symbol=payload_symbol,
            price=price,
            size=size,
            side=str(msg.get("side", "")),
            trade_id=int(tid) if isinstance(tid, (int, float)) else None,
            received_ns=received_ns,
            ex_timestamp=int(msg.get("ex_timestamp", 0) or 0),
            recv_timestamp=int(msg.get("recv_timestamp", 0) or 0),
            full_slug=full_slug if isinstance(full_slug, str) else None,
        )
        if self._on_trade is None:
            return
        try:
            self._on_trade(exchange, topic_id, t)
        except Exception:  # noqa: BLE001
            log.exception("on_trade handler raised")


def _coerce_levels(raw: object) -> list[tuple[float, float]]:
    """Convert an `OrderbookSnapshot.bids`/`asks` payload into a clean
    `list[(px, qty)]`. Each level is `[price, size]` in the msgpack
    encoding. Drops malformed entries silently (best-effort)."""
    if not isinstance(raw, list):
        return []
    out: list[tuple[float, float]] = []
    for lvl in raw:
        if not isinstance(lvl, (list, tuple)) or len(lvl) < 2:
            continue
        try:
            out.append((float(lvl[0]), float(lvl[1])))
        except (TypeError, ValueError):
            continue
    return out
