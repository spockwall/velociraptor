"""End-to-end latency tracer for the order path.

Threads one timestamp chain per order from "strategy decided to send" all the
way to "the Polymarket order shows up in our own book", and writes one CSV row
per order. Two independent "it landed" signals are recorded:

  - **BBA-match** (primary, fast): the first Polymarket BBA frame *after* the
    order whose book changed on the order's symbol. Cheap and quick, but can't
    *prove* the change was ours vs. market noise.
  - **fill event** (cross-check, slow): the UserFeed `fill`/`order_update` for
    the order's `client_oid`. Authoritative attribution, but the venue's user
    channel is slower than the public book.

## The timestamp chain (all ns since UNIX epoch)

Market-data leg (carried onto the triggering `Quote`, set upstream):
  t_exch  — venue event time (0 if the venue gave none, e.g. Binance spot depth)
  t_recv  — orderbook_server read the WS frame
  t_pub   — (implicit; ~t_recv, the server publishes immediately)
  t_eng   — engine received the BBA frame (`Quote.received_ns`)

Order leg (this tracer + executor `meta`):
  t_decide     — strategy called place_* (engine clock, just before send)
  t_exec_recv  — executor decoded the req off ROUTER     (executor clock)
  t_exec_send  — executor about to POST to the venue      (executor clock)
  t_ack        — executor got the venue ack               (executor clock)
  t_resp       — engine received the response             (engine clock)
  t_bba_match  — first post-order Polymarket BBA change   (engine clock)
  t_fill_evt   — UserFeed fill for this client_oid         (engine clock)

## Clock-skew honesty

Everything on the **engine clock** (t_decide, t_resp, t_bba_match, t_fill_evt)
is mutually skew-free — these give the numbers you actually care about
("decide → landed"). The executor stamps share *one* clock too, so
`t_ack - t_exec_send` (the real venue REST round-trip) and
`t_exec_send - t_exec_recv` (executor overhead) are skew-free. Only hops that
*cross* machines mix clocks: `t_exec_recv - t_decide` (engine→executor
transport) and the t_recv/t_exch market-data wire times. Run NTP/chrony and
read those cross-machine deltas as indicative, not exact.

Negative cross-machine deltas (clock skew) are written as-is — don't silently
clamp; a negative number is itself a signal that the clocks disagree.
"""

from __future__ import annotations

import csv
import dataclasses
import logging
import os
import threading
import time
from typing import Optional

log = logging.getLogger(__name__)

# After this long with no BBA-match / fill, flush the row anyway with whatever
# we have (the missing stamps stay 0). Keeps the in-flight map from leaking.
_INFLIGHT_TIMEOUT_NS = 30 * 1_000_000_000  # 30s


@dataclasses.dataclass
class _Inflight:
    """One order awaiting its landed-signals. Mutated by callbacks from
    several threads — guard all access with the tracer lock."""

    client_oid: str
    exchange_oid: str
    exchange: str
    symbol: str
    side: str
    kind: str  # "limit" | "market"
    px: float  # order price (0.0 for a market order — price is irrelevant)
    qty: float  # order size (shares for sell/limit; USDC notional for buy market)
    # market-data leg of the quote that triggered this order (may be 0s if the
    # strategy didn't pass a triggering quote)
    t_exch_ns: int
    t_recv_ns: int
    t_eng_ns: int
    # order leg
    t_decide_ns: int
    # Baseline book at order time, captured from the last frame seen on this
    # symbol *before* the order: {price: qty} for each side. Used to detect the
    # specific change our order should cause (see `_book_reflects`).
    base_bids: dict = dataclasses.field(default_factory=dict)
    base_asks: dict = dataclasses.field(default_factory=dict)
    t_resp_ns: int = 0
    t_exec_recv_ns: int = 0
    t_exec_send_ns: int = 0
    t_ack_ns: int = 0
    t_bba_match_ns: int = 0  # first book frame after order (timing only)
    t_book_match_ns: int = 0  # first frame the book actually reflects our order
    matched: int = -1  # -1 unknown, 1 book confirmed our order, 0 timed out unconfirmed
    match_px: float = 0.0  # the book price where we saw our order's effect
    match_dqty: float = 0.0  # the qty delta we observed at that price (signed)
    t_fill_evt_ns: int = 0
    ok: bool = True

    def done(self) -> bool:
        """Ready to flush once the book confirms the order reflects in it
        (t_book_match) — that's the signal you care about. The slow user-channel
        fill is an opportunistic cross-check: it's included if it already
        arrived, otherwise the row flushes without it (t_fill_evt stays 0).
        Orders never confirmed flush on the 30s timeout with matched=0."""
        return self.t_book_match_ns != 0


# CSV columns, in order. Stable so `analyze_latency.py` can rely on them.
_COLUMNS = [
    "wall_iso",
    "exchange",
    "symbol",
    "side",
    "kind",
    "client_oid",
    "exchange_oid",
    "ok",
    "px",
    "qty",
    # did the orderbook actually reflect our order? (price/qty check)
    "matched",  # 1 confirmed, 0 timed-out unconfirmed, -1 unknown
    "match_px",  # book price where we observed our order's effect
    "match_dqty",  # observed qty change at that price (signed)
    # raw stamps (ns since epoch)
    "t_exch_ns",
    "t_recv_ns",
    "t_eng_ns",
    "t_decide_ns",
    "t_exec_recv_ns",
    "t_exec_send_ns",
    "t_ack_ns",
    "t_resp_ns",
    "t_bba_match_ns",
    "t_book_match_ns",
    "t_fill_evt_ns",
    # derived per-stage deltas (ms). Blank when an endpoint stamp is missing.
    "md_wire_ms",  # t_recv - t_exch     (venue→server, cross-clock)
    "md_transport_ms",  # t_eng  - t_recv     (server→engine)
    "eng_to_exec_ms",  # t_exec_recv - t_decide (engine→executor, cross-clock)
    "exec_overhead_ms",  # t_exec_send - t_exec_recv (executor internal)
    "venue_rtt_ms",  # t_ack - t_exec_send (the exchange REST round-trip)
    "resp_return_ms",  # t_resp - t_ack      (executor→engine return)
    "order_rtt_ms",  # t_resp - t_decide   (full engine-side order round-trip)
    "bba_match_ms",  # t_bba_match - t_decide (decide → next book frame, timing-only)
    "book_match_ms",  # t_book_match - t_decide (decide → book CONFIRMED our order)
    "fill_evt_ms",  # t_fill_evt  - t_decide (decide → user fill, SLOW)
]


def _ms(a: int, b: int) -> Optional[float]:
    """(`a` - `b`) in ms, or None if either endpoint stamp is missing (0)."""
    if not a or not b:
        return None
    return (a - b) / 1e6


# Polymarket prices are on a 0.001 (or 0.01) grid; match the order price to a
# book level within this tolerance. Qty must move by at least this many shares
# to count as a real change (filters float noise).
_PX_TOL = 0.005
_QTY_EPS = 1e-6


def _book_reflects(item: "_Inflight", bids: dict, asks: dict):
    """Did this order's effect show up in the book? Returns `(price, dqty)` of
    the observed change, or `None` if the book doesn't yet reflect the order.

    What "reflects" means depends on side × kind:

      * **buy market**  → consumes asks: best/asks qty at/above touch SHRANK
                          vs. baseline. (a buy eats the ask side)
      * **sell market** → consumes bids: bid qty SHRANK vs. baseline.
      * **buy limit**   → rests on the bid side: a bid level at our price
                          APPEARED or GREW by ~our qty.
      * **sell limit**  → rests on the ask side: an ask level at our price
                          APPEARED or GREW.

    A market order that fully fills may also leave a resting remainder; we
    accept either signal (liquidity removed on the far side OR a level added on
    our side). The check is deliberately lenient on exact qty — venues round,
    partial-fill, and other traders move size too — so we confirm DIRECTION and
    that the moved qty is non-trivial, and record the actual delta for the CSV
    so you can eyeball whether it's ~your size."""
    is_buy = item.side.lower() in ("buy", "bid", "up")

    if item.kind == "market":
        # Liquidity should have been removed from the far (taker) side.
        far_now, far_base = (asks, item.base_asks) if is_buy else (bids, item.base_bids)
        # Look for the largest shrink across price levels present in baseline.
        best = None  # (px, dqty) with most-negative dqty
        for px, base_q in far_base.items():
            now_q = far_now.get(px, 0.0)
            dq = now_q - base_q  # negative = liquidity removed
            if dq < -_QTY_EPS and (best is None or dq < best[1]):
                best = (px, dq)
        if best is not None:
            return best
        # Fallback: a resting remainder appeared on our own side.
        near_now, near_base = (bids, item.base_bids) if is_buy else (asks, item.base_asks)
        for px, now_q in near_now.items():
            dq = now_q - near_base.get(px, 0.0)
            if dq > _QTY_EPS:
                return (px, dq)
        return None

    # Limit order: a level at ~our price should have appeared or grown on our side.
    near_now, near_base = (bids, item.base_bids) if is_buy else (asks, item.base_asks)
    for px, now_q in near_now.items():
        if abs(px - item.px) > _PX_TOL:
            continue
        dq = now_q - near_base.get(px, 0.0)
        if dq > _QTY_EPS:
            return (px, dq)
    return None


class LatencyTracer:
    """Per-order latency recorder. Thread-safe: the order callbacks run on the
    strategy thread while `on_poly_book`/`on_fill` run on the feed threads.

    Wire-up (see `app.py`):
      - OrderRouter: call `on_order_sent(...)` after each place ack (with the
        order's px/qty so the book-confirm can verify it).
      - MarketFeed (Polymarket snapshot cb): call
        `on_poly_book(symbol, bids, asks, recv_ns)`.
      - UserFeed (fill/order_update): call `on_fill(client_oid, recv_ns)`.
    """

    def __init__(self, csv_path: str):
        self._path = csv_path
        self._lock = threading.Lock()
        # in-flight orders keyed by client_oid (we generate it; most reliable)
        self._inflight: dict[str, _Inflight] = {}
        # exchange_oid → client_oid, for signals that only carry exchange_oid
        self._oid_index: dict[str, str] = {}
        # last full book seen per symbol: (bids{px:qty}, asks{px:qty}). The
        # snapshot at order time becomes that order's match baseline.
        self._last_book: dict[str, tuple[dict, dict]] = {}
        os.makedirs(os.path.dirname(os.path.abspath(csv_path)) or ".", exist_ok=True)
        new = not os.path.exists(csv_path) or os.path.getsize(csv_path) == 0
        # line-buffered so rows hit disk as orders complete
        self._fh = open(csv_path, "a", newline="", buffering=1)
        self._writer = csv.writer(self._fh)
        if new:
            self._writer.writerow(_COLUMNS)
        log.info(f"latency tracer → {csv_path}")

    # ── order leg ──

    def on_order_sent(
        self,
        *,
        client_oid: str,
        exchange_oid: str,
        exchange: str,
        symbol: str,
        side: str,
        kind: str,
        px: float,
        qty: float,
        t_decide_ns: int,
        t_resp_ns: int,
        meta: Optional[dict],
        ok: bool,
        quote=None,
    ) -> None:
        """Register an order after its place ack. `meta` is the executor
        `OrderMeta` map (`t_exec_recv_ns`/`t_exec_send_ns`/`t_ack_ns`); `quote`
        is the triggering market-data `Quote`. `px`/`qty` are the order's price
        and size — used to confirm a later book frame actually reflects THIS
        order (price/qty check), not just any post-order frame."""
        meta = meta or {}
        with self._lock:
            base = self._last_book.get(symbol, ({}, {}))
            item = _Inflight(
                client_oid=client_oid,
                exchange_oid=exchange_oid or "",
                exchange=exchange,
                symbol=symbol,
                side=side,
                kind=kind,
                px=float(px or 0.0),
                qty=float(qty or 0.0),
                # snapshot the pre-order book as this order's match baseline
                base_bids=dict(base[0]),
                base_asks=dict(base[1]),
                t_exch_ns=int(getattr(quote, "t_exch_ns", 0) or 0),
                t_recv_ns=int(getattr(quote, "t_recv_ns", 0) or 0),
                t_eng_ns=int(getattr(quote, "received_ns", 0) or 0),
                t_decide_ns=t_decide_ns,
                t_resp_ns=t_resp_ns,
                t_exec_recv_ns=int(meta.get("t_exec_recv_ns", 0) or 0),
                t_exec_send_ns=int(meta.get("t_exec_send_ns", 0) or 0),
                t_ack_ns=int(meta.get("t_ack_ns", 0) or 0),
                ok=ok,
            )
            # A failed order can't land in the book — flush immediately so it
            # still appears in the CSV (with the order-leg stamps it has).
            if not ok:
                self._flush_locked(item)
                return
            self._inflight[client_oid] = item
            if exchange_oid:
                self._oid_index[exchange_oid] = client_oid
            self._expire_locked()

    # ── landed signals ──

    def on_poly_book(
        self,
        symbol: str,
        bids: list,
        asks: list,
        received_ns: int,
    ) -> None:
        """A Polymarket full-depth frame for `symbol` (lists of (px, qty)).
        Updates the per-symbol book, then for each in-flight order on this
        symbol:
          - stamps `t_bba_match` on the first post-order frame (timing only —
            "the book moved at all", may be unrelated to us);
          - runs the price/qty check; if the book change matches what THIS
            order should cause, stamps `t_book_match` and records the
            observed price + qty delta (the real confirmation)."""
        cur_bids = {float(p): float(q) for p, q in bids}
        cur_asks = {float(p): float(q) for p, q in asks}
        with self._lock:
            self._last_book[symbol] = (cur_bids, cur_asks)
            ready = []
            for item in self._inflight.values():
                if item.symbol != symbol or received_ns <= item.t_decide_ns:
                    continue
                if item.t_bba_match_ns == 0:
                    item.t_bba_match_ns = received_ns
                if item.t_book_match_ns == 0:
                    hit = _book_reflects(item, cur_bids, cur_asks)
                    if hit is not None:
                        item.t_book_match_ns = received_ns
                        item.matched = 1
                        item.match_px, item.match_dqty = hit
                if item.done():
                    ready.append(item)
            for item in ready:
                self._pop_and_flush_locked(item)

    def on_fill(
        self,
        *,
        client_oid: Optional[str],
        exchange_oid: Optional[str],
        received_ns: int,
    ) -> None:
        """A UserFeed fill / order_update arrived. Match it to an in-flight
        order by client_oid (preferred) or exchange_oid, and stamp
        t_fill_evt."""
        with self._lock:
            item = None
            if client_oid and client_oid in self._inflight:
                item = self._inflight[client_oid]
            elif exchange_oid:
                coid = self._oid_index.get(exchange_oid)
                if coid:
                    item = self._inflight.get(coid)
            if item is None or item.t_fill_evt_ns != 0:
                return
            item.t_fill_evt_ns = received_ns
            if item.done():
                self._pop_and_flush_locked(item)

    # ── shutdown ──

    def close(self) -> None:
        """Flush all still-in-flight orders (partial rows) and close the file."""
        with self._lock:
            for item in list(self._inflight.values()):
                if item.ok and item.matched == -1:
                    item.matched = 0  # never confirmed before shutdown
                self._flush_locked(item)
            self._inflight.clear()
            self._oid_index.clear()
            try:
                self._fh.close()
            except Exception:  # noqa: BLE001
                pass

    # ── inner (lock held) ──

    def _expire_locked(self) -> None:
        now = time.time_ns()
        stale = [
            it
            for it in self._inflight.values()
            if now - it.t_decide_ns > _INFLIGHT_TIMEOUT_NS
        ]
        for item in stale:
            # Timed out before the book confirmed our order: record matched=0
            # (unconfirmed) unless a confirm already landed.
            if item.ok and item.matched == -1:
                item.matched = 0
            self._pop_and_flush_locked(item)

    def _pop_and_flush_locked(self, item: _Inflight) -> None:
        self._inflight.pop(item.client_oid, None)
        if item.exchange_oid:
            self._oid_index.pop(item.exchange_oid, None)
        self._flush_locked(item)

    def _flush_locked(self, it: _Inflight) -> None:
        row = [
            time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()),
            it.exchange,
            it.symbol,
            it.side,
            it.kind,
            it.client_oid,
            it.exchange_oid,
            int(it.ok),
            _fmt(it.px),
            _fmt(it.qty),
            it.matched,
            _fmt(it.match_px),
            _fmt(it.match_dqty),
            it.t_exch_ns,
            it.t_recv_ns,
            it.t_eng_ns,
            it.t_decide_ns,
            it.t_exec_recv_ns,
            it.t_exec_send_ns,
            it.t_ack_ns,
            it.t_resp_ns,
            it.t_bba_match_ns,
            it.t_book_match_ns,
            it.t_fill_evt_ns,
            _fmt(_ms(it.t_recv_ns, it.t_exch_ns)),
            _fmt(_ms(it.t_eng_ns, it.t_recv_ns)),
            _fmt(_ms(it.t_exec_recv_ns, it.t_decide_ns)),
            _fmt(_ms(it.t_exec_send_ns, it.t_exec_recv_ns)),
            _fmt(_ms(it.t_ack_ns, it.t_exec_send_ns)),
            _fmt(_ms(it.t_resp_ns, it.t_ack_ns)),
            _fmt(_ms(it.t_resp_ns, it.t_decide_ns)),
            _fmt(_ms(it.t_bba_match_ns, it.t_decide_ns)),
            _fmt(_ms(it.t_book_match_ns, it.t_decide_ns)),
            _fmt(_ms(it.t_fill_evt_ns, it.t_decide_ns)),
        ]
        try:
            self._writer.writerow(row)
        except Exception:  # noqa: BLE001
            log.exception("latency tracer write failed")


def _fmt(v: Optional[float]) -> str:
    """CSV cell for an optional ms delta: blank if missing, else 3-dp."""
    return "" if v is None else f"{v:.3f}"
