"""Momentum — Binance signal → one small Polymarket position.

Event-driven version. Two quote callbacks:

  - Binance (~50ms throttled) — keeps a samples deque current. The
    throttle prevents 200×/sec fast quotes from translating to 200
    strategy invocations; `MarketState.mid("binance", bsym)` always
    stays fresh between fires because MarketState is updated
    unconditionally before the throttle check.
  - Polymarket UP (no throttle) — runs the entry/exit/manage-open
    decision tree. Window-phase guard ("force-exit in the last N secs"
    of the window) is checked here, since it's gated on Polymarket
    quotes anyway and no on_timer event exists in this engine.

Order reconciliation flows through `on_fill` / `on_order_update`.
"""

from __future__ import annotations

import logging
import time
from collections import deque
from typing import Deque, Optional, Tuple

from ...market import Quote
from ...typings.state import OrderRecord
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import TICK, clamp_px, qty_for_notional

log = logging.getLogger(__name__)

# Tunables (hardcoded — single tuning surface for the validation run).
MOMENTUM_LOOKBACK_SECS = 20
MOMENTUM_THRESHOLD_BPS = 5.0
ORDER_NOTIONAL_USD = 1.0
ENTRY_CROSS_TICKS = 1
EXIT_CROSS_TICKS = 1
TP_PCT = 0.08
SL_PCT = 0.05
SKIP_FIRST_SECS = 60
SKIP_LAST_SECS = 120
SAMPLE_MAX = 600

# Throttle the Binance sampler so a fast feed doesn't burn CPU.
_BINANCE_QUOTE_MIN_MS = 50.0


def _binance_symbol_for(base_slug: str) -> Optional[str]:
    """Map a Polymarket base slug to the Binance feed symbol used for
    the momentum signal. Unknown → None (strategy stays inert)."""
    s = base_slug.lower()
    if s.startswith("btc-"):
        return "btcusdt"
    if s.startswith("eth-"):
        return "ethusdt"
    return None


class MomentumStrategy(Strategy):
    name = "momentum"

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        if self.window is None:
            raise ValueError("MomentumStrategy needs a Polymarket window")
        self.bsym = _binance_symbol_for(self.window.base_slug)
        self._samples: Deque[Tuple[float, float]] = deque(maxlen=SAMPLE_MAX)
        # Single-position state.
        self.entry_oid: Optional[str] = None
        self.entry_px: Optional[float] = None
        self.entry_qty: Optional[float] = None
        self.closing: bool = False
        if self.bsym is None:
            log.warning(
                f"[{self.label}] momentum: no Binance symbol for "
                f"base_slug={self.window.base_slug!r} — strategy inert"
            )

    def required_topics(self) -> list[tuple[str, str, str]]:
        topics: list[tuple[str, str, str]] = [
            ("polymarket", self.window.base_slug, "snapshot"),
            ("polymarket", self.window.base_slug, "trade"),
        ]
        if self.bsym is not None:
            topics.append(("binance", self.bsym, "snapshot"))
            topics.append(("binance", self.bsym, "trade"))
        return topics

    def setup(self, dispatcher: Dispatcher) -> None:
        if self.bsym is not None:
            dispatcher.register_quote(
                "binance",
                self.bsym,
                self._on_binance_quote,
                min_interval_ms=_BINANCE_QUOTE_MIN_MS,
            )
        # Polymarket quote drives entry/exit/manage-open — fire on every
        # frame (overrides dispatcher's 2s default throttle).
        dispatcher.register_quote(
            "polymarket",
            self.window.base_slug,
            self._on_poly_quote,
            min_interval_ms=0,
        )
        dispatcher.register_rollover(
            "polymarket", self.window.base_slug, self._on_rollover
        )
        dispatcher.register_bootstrap(
            "polymarket", self.window.base_slug, self._on_bootstrap
        )
        dispatcher.register_order_update(self._on_order_update)
        dispatcher.register_fill(self._on_fill)

    # ── handlers ──

    def _on_bootstrap(self, full_slug: str) -> None:
        # No-op stub. MarketState.on_bootstrap already populated
        # asset_ids; the engine pre-stamped self.window.full_slug.
        # Crucially we do NOT call `_reset_position` here — there's
        # no prior position to clear at process start.
        pass

    def _on_rollover(self, full_slug: str) -> None:
        # asset_ids already resolved by MarketState.on_rollover before
        # this callback fires. New window → flat.
        self.window.full_slug = full_slug
        self._reset_position()

    def _on_binance_quote(self, q: Quote) -> None:
        if q.mid is not None:
            self._samples.append((time.time(), q.mid))

    def _on_poly_quote(self, _yq: Quote) -> None:
        if self.bsym is None:
            return
        now = time.time()
        phase = self._phase(now)
        signal = self._momentum_bps(now)
        yq = self.market.quote("polymarket", self.window.base_slug)

        action = "skip"
        try:
            if self.entry_oid is not None or self.closing:
                action = self._manage_open(now, phase)
            elif phase == "bootstrapping" or self._asset_id("up") is None:
                action = "skip(bootstrapping)"
            elif phase == "late":
                action = "skip(late)"
            elif phase == "early":
                action = "skip(early)"
            elif signal is None:
                action = "skip(warming)"
            elif abs(signal) < MOMENTUM_THRESHOLD_BPS:
                action = "skip(flat)"
            elif signal < 0:
                # Server forwards only the UP token — going short the
                # NO side isn't expressible. Stand aside.
                action = "skip(no-side-not-tradable)"
            else:
                action = self._enter(now)
        finally:
            self._log_feed(now, yq, signal, phase, action)

    def _on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None or oid != self.entry_oid:
            return
        if status in {"canceled", "filled", "rejected", "expired"}:
            self.state.orders.mark(oid, status)
            log.info(f"[{self.label}] MOM order terminal status={status} oid={oid}")
            if self.closing or status in {"canceled", "rejected", "expired"}:
                self._reset_position()

    def _on_fill(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        if oid is None or oid != self.entry_oid:
            return
        log.info(
            f"[{self.label}] MOM FILL px={ev.get('px')} "
            f"qty={ev.get('qty')} oid={oid}"
        )
        if self.closing:
            self._reset_position()

    # ── helpers ──

    def _phase(self, now: float) -> str:
        ws = self.window.window_start
        we = self.window.window_end
        if ws is None or we is None:
            return "bootstrapping"
        elapsed = now - ws
        remaining = we - now
        if elapsed < SKIP_FIRST_SECS:
            return "early"
        if remaining <= SKIP_LAST_SECS:
            return "late"
        return "active"

    def _momentum_bps(self, now: float) -> Optional[float]:
        if not self._samples:
            return None
        _latest_ts, latest_mid = self._samples[-1]
        cutoff = now - MOMENTUM_LOOKBACK_SECS
        ref: Optional[float] = None
        for ts, mid in self._samples:
            if ts <= cutoff:
                ref = mid
            else:
                break
        if ref is None or ref <= 0:
            return None
        return (latest_mid - ref) / ref * 1e4

    def _enter(self, now: float) -> str:
        asset_id = self._asset_id("up")
        if asset_id is None:
            return "skip(no asset_id)"
        q = self.market.quote("polymarket", self.window.base_slug)
        if q is None or q.best_ask is None:
            return "skip(no quote)"
        px = clamp_px(q.best_ask + ENTRY_CROSS_TICKS * TICK)
        qty = qty_for_notional(ORDER_NOTIONAL_USD, px)
        if qty <= 0:
            return "skip(qty<=0)"
        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-mom-{slug[:18]}-{int(now * 1000)}"
        try:
            ack = self.router.place_limit(
                symbol=asset_id,
                side="buy",
                px=px,
                qty=qty,
                client_oid=client_oid,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] MOM enter place failed: {e}")
            return "skip(place-failed)"
        self.entry_oid = ack.get("exchange_oid")
        self.entry_px = px
        self.entry_qty = qty
        self.state.orders.add(
            OrderRecord(
                client_oid=client_oid,
                side_name="up",
                symbol=asset_id,
                direction="buy",
                px=px,
                qty=qty,
                exchange_oid=self.entry_oid,
            )
        )
        log.info(
            f"[{self.label}] MOM ENTER px={px:.2f} qty={qty:.2f} "
            f"oid={self.entry_oid}"
        )
        return "ENTER up"

    def _manage_open(self, now: float, phase: str) -> str:
        if self.closing:
            return "closing"
        if self.entry_px is None or self._asset_id("up") is None:
            return "hold"
        q = self.market.quote("polymarket", self.window.base_slug)
        if phase == "late":
            return self._exit(q, "window-exit")
        if q is None or q.mid is None or self.entry_px <= 0:
            return "hold(no quote)"
        move = (q.mid - self.entry_px) / self.entry_px
        if move >= TP_PCT:
            return self._exit(q, "TP")
        if move <= -SL_PCT:
            return self._exit(q, "SL")
        return "HOLD"

    def _exit(self, q: Optional[Quote], reason: str) -> str:
        asset_id = self._asset_id("up")
        if asset_id is None:
            return "hold"
        if q is None or q.best_bid is None:
            log.warning(f"[{self.label}] MOM {reason} exit: no bid yet")
            return f"{reason}(no-bid)"
        px = clamp_px(q.best_bid - EXIT_CROSS_TICKS * TICK)
        qty = self.entry_qty or qty_for_notional(ORDER_NOTIONAL_USD, px)
        slug = self.window.full_slug or self.window.base_slug
        client_oid = f"te-mom-{slug[:18]}-exit-{int(time.time() * 1000)}"
        try:
            ack = self.router.place_limit(
                symbol=asset_id,
                side="sell",
                px=px,
                qty=qty,
                client_oid=client_oid,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] MOM {reason} exit place failed: {e}")
            return f"{reason}(exit-failed)"
        self.closing = True
        self.state.orders.add(
            OrderRecord(
                client_oid=client_oid,
                side_name="up-exit",
                symbol=asset_id,
                direction="sell",
                px=px,
                qty=qty,
                exchange_oid=ack.get("exchange_oid"),
            )
        )
        log.info(
            f"[{self.label}] MOM {reason} exit px={px:.2f} qty={qty:.2f} "
            f"oid={ack.get('exchange_oid')}"
        )
        return f"{reason} exit"

    def _reset_position(self) -> None:
        self.entry_oid = None
        self.entry_px = None
        self.entry_qty = None
        self.closing = False

    def _log_feed(
        self,
        now: float,
        yq: Optional[Quote],
        signal: Optional[float],
        phase: str,
        action: str,
    ) -> None:
        if self.bsym is None:
            binance = "binance:NONE"
        else:
            bq = self.market.quote("binance", self.bsym)
            btr = self.market.trade("binance", self.bsym)
            if bq is None:
                binance = "binance:MISSING"
            else:
                age = (time.time_ns() - bq.received_ns) / 1e9
                binance = (
                    f"binance:{self.bsym} bid={_f(bq.best_bid)} ask={_f(bq.best_ask)} "
                    f"mid={_f(bq.mid)} age={age:.1f}s"
                )
            if btr is None:
                binance += " trade=—"
            else:
                t_age = (time.time_ns() - btr.received_ns) / 1e9
                binance += (
                    f" trade={_f(btr.price)}x{_f(btr.size)} {btr.side} age={t_age:.1f}s"
                )
        poly = (
            f"up(b/a)={_f(_b(yq))}/{_f(_a(yq))}" if yq is not None else "poly:MISSING"
        )
        sig = "warming" if signal is None else f"{signal:+.1f}bps"
        if self.entry_oid is None:
            pos = "none"
        else:
            pos = f"up@{_f(self.entry_px)}x{_f(self.entry_qty)}"
            if self.closing:
                pos += "(closing)"
        log.info(
            f"[{self.label}] FEED {binance} | {poly} | "
            f"signal={sig} phase={phase} pos={pos} action={action}"
        )

    def teardown(self) -> None:
        if self.entry_oid is not None:
            try:
                self.router.cancel(self.entry_oid, **self._attribution())
            except Exception as e:  # noqa: BLE001
                log.warning(f"[{self.label}] MOM teardown cancel failed: {e}")
        self._reset_position()


def _f(v: Optional[float]) -> str:
    return f"{v:.4f}" if isinstance(v, (int, float)) else "—"


def _b(q: Optional[Quote]) -> Optional[float]:
    return q.best_bid if q is not None else None


def _a(q: Optional[Quote]) -> Optional[float]:
    return q.best_ask if q is not None else None
