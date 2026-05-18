"""Momentum strategy — Binance signal → one small Polymarket token position.

Purpose of this strategy is **dual-feed + fills-WS validation**: it reads
Binance btc/eth orderbook for a short-window momentum signal and, when the
signal is strong enough, takes ONE small position on the matching Polymarket
15m up/down token, then manages TP/SL itself (Polymarket has no stop orders).

Every tick it logs both feeds side-by-side (`FEED ...`) so you can eyeball
whether Binance *and* Polymarket data stay live and consistent at the same
time, and whether `MOM FILL` events actually arrive over the user channel.

Window-phase guard: no new entry in the first `SKIP_FIRST_SECS` or the last
`SKIP_LAST_SECS` of the 15m window; in the last `SKIP_LAST_SECS` any open
position is force-closed.

All knobs are hardcoded below — this is intentionally a single-file, single
place to tune for the validation run. Run small, on testnet, until the fills
WebSocket is confirmed reliable.
"""

from __future__ import annotations

import logging
import time
from collections import deque
from typing import Deque, Optional, Tuple

from ...market import Quote, Trade
from .base import TICK, Strategy, clamp_px, qty_for_notional

log = logging.getLogger(__name__)

# ── Tunables (hardcoded by design) ───────────────────────────────────────────
MOMENTUM_LOOKBACK_SECS = 20      # Binance mid now vs mid ~N s ago
MOMENTUM_THRESHOLD_BPS = 5.0     # |Δ| in bps to take a side; below → skip
ORDER_NOTIONAL_USD = 1.0         # small, by design (validation run)
ENTRY_CROSS_TICKS = 1            # entry limit this many ticks past the ask
EXIT_CROSS_TICKS = 1             # exit limit this many ticks below the bid
TP_PCT = 0.08                    # +8% on token price → take profit
SL_PCT = 0.05                    # -5% on token price → stop loss
SKIP_FIRST_SECS = 60             # no entry in the first 1 min of the window
SKIP_LAST_SECS = 120             # no entry + force-exit in the last 2 min
SAMPLE_MAX = 600                 # cap the momentum sample deque


def _binance_symbol_for(base_slug: str) -> Optional[str]:
    """Map a Polymarket base slug to the Binance feed symbol used for the
    momentum signal. Unknown → None (strategy no-ops that window)."""
    s = base_slug.lower()
    if s.startswith("btc-"):
        return "btcusdt"
    if s.startswith("eth-"):
        return "ethusdt"
    return None


class MomentumStrategy(Strategy):
    name = "momentum"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.bsym = _binance_symbol_for(self.window.base_slug)
        # (ts_secs, binance_mid) samples for the momentum window.
        self._samples: Deque[Tuple[float, float]] = deque(maxlen=SAMPLE_MAX)
        # Single-position state.
        self.entry_oid: Optional[str] = None
        self.entry_side: Optional[str] = None  # "yes" | "no"
        self.entry_px: Optional[float] = None
        self.entry_qty: Optional[float] = None
        self.closing: bool = False
        if self.bsym is None:
            log.warning(
                "[%s] momentum: no Binance symbol for base_slug=%r — window inert",
                self.label,
                self.window.base_slug,
            )

    # ── abstract hook (unused: we override tick() for single-position flow) ──
    def _target_px(self, quote: Quote):  # pragma: no cover - not used
        return None

    # ── helpers ──
    def _asset_for(self, side: str) -> str:
        return self.yes.asset_id if side == "yes" else self.no.asset_id

    def _phase(self, now: float) -> str:
        elapsed = now - self.window.window_start
        remaining = self.window.window_end - now
        if elapsed < SKIP_FIRST_SECS:
            return "early"
        if remaining <= SKIP_LAST_SECS:
            return "late"
        return "active"

    def _momentum_bps(self, now: float) -> Optional[float]:
        """Δ in bps between the latest Binance mid and the mid ~lookback ago.
        None until enough history has accumulated."""
        if not self._samples:
            return None
        latest_ts, latest_mid = self._samples[-1]
        cutoff = now - MOMENTUM_LOOKBACK_SECS
        ref: Optional[float] = None
        for ts, mid in self._samples:
            if ts <= cutoff:
                ref = mid  # last sample at/older than the cutoff
            else:
                break
        if ref is None or ref <= 0:
            return None
        return (latest_mid - ref) / ref * 1e4

    # ── lifecycle ──
    def tick(self) -> None:
        now = time.time()
        if self.bsym is None:
            return

        bq = self.feed.latest(self.bsym, exchange="binance")
        if bq is not None and bq.mid is not None:
            self._samples.append((now, bq.mid))
        # Public Binance trade prints (last executed price/size/side). Used
        # only for the FEED log line + as a fallback momentum reference when
        # the BBA mid is briefly unavailable.
        btr = self.feed.latest_trade(self.bsym, exchange="binance")
        if (bq is None or bq.mid is None) and btr is not None:
            self._samples.append((now, btr.price))

        phase = self._phase(now)
        signal = self._momentum_bps(now)

        # Snapshot both Polymarket token quotes for the FEED log line.
        yq = self.feed.latest(self.yes.asset_id)
        nq = self.feed.latest(self.no.asset_id)

        action = "skip"
        try:
            if self.entry_oid is not None or self.closing:
                action = self._manage_open(now, phase)
            elif phase == "late":
                action = "skip(late)"
            elif phase == "early":
                action = "skip(early)"
            elif signal is None:
                action = "skip(warming)"
            elif abs(signal) < MOMENTUM_THRESHOLD_BPS:
                action = "skip(flat)"
            else:
                side = "yes" if signal > 0 else "no"
                action = self._enter(side, now)
        finally:
            self._log_feed(now, bq, btr, yq, nq, signal, phase, action)

    def _enter(self, side: str, now: float) -> str:
        asset = self._asset_for(side)
        q = self.feed.latest(asset)
        if q is None or q.best_ask is None:
            return f"skip(no {side} quote)"
        px = clamp_px(q.best_ask + ENTRY_CROSS_TICKS * TICK)
        qty = qty_for_notional(ORDER_NOTIONAL_USD, px)
        if qty <= 0:
            return "skip(qty<=0)"
        client_oid = f"te-mom-{self.window.full_slug[:18]}-{side}-{int(now * 1000)}"
        try:
            ack = self.router.place_limit(
                symbol=asset,
                side="buy",
                px=px,
                qty=qty,
                client_oid=client_oid,
                **self._attribution(side),
            )
        except Exception as e:  # noqa: BLE001
            log.warning("[%s] MOM enter %s place failed: %s", self.label, side, e)
            return "skip(place-failed)"
        self.entry_oid = ack.get("exchange_oid")
        self.entry_side = side
        self.entry_px = px
        self.entry_qty = qty
        log.info(
            "[%s] MOM ENTER %s px=%.2f qty=%.2f oid=%s",
            self.label,
            side,
            px,
            qty,
            self.entry_oid,
        )
        return f"ENTER {side}"

    def _manage_open(self, now: float, phase: str) -> str:
        """Position (or close order) is live. Force-exit late; else apply
        TP/SL on the held token's mid vs entry price."""
        if self.closing:
            return "closing"
        if self.entry_side is None or self.entry_px is None:
            return "hold"
        asset = self._asset_for(self.entry_side)
        q = self.feed.latest(asset)
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
        """Flatten by selling the held token back near best_bid (we only ever
        buy tokens; the close is the opposite-direction order)."""
        if self.entry_side is None:
            return "hold"
        asset = self._asset_for(self.entry_side)
        if q is None or q.best_bid is None:
            # No book to price the exit — retry next tick (window-end backstop
            # will keep firing this path).
            log.warning("[%s] MOM %s exit: no bid yet, retrying", self.label, reason)
            return f"{reason}(no-bid)"
        px = clamp_px(q.best_bid - EXIT_CROSS_TICKS * TICK)
        qty = self.entry_qty or qty_for_notional(ORDER_NOTIONAL_USD, px)
        client_oid = f"te-mom-{self.window.full_slug[:18]}-exit-{int(time.time() * 1000)}"
        try:
            ack = self.router.place_limit(
                symbol=asset,
                side="sell",
                px=px,
                qty=qty,
                client_oid=client_oid,
                **self._attribution(f"{self.entry_side}-exit"),
            )
        except Exception as e:  # noqa: BLE001
            log.warning("[%s] MOM %s exit place failed: %s", self.label, reason, e)
            return f"{reason}(exit-failed)"
        self.closing = True
        log.info(
            "[%s] MOM %s exit %s px=%.2f qty=%.2f oid=%s",
            self.label,
            reason,
            self.entry_side,
            px,
            qty,
            ack.get("exchange_oid"),
        )
        return f"{reason} exit"

    def _reset_position(self) -> None:
        self.entry_oid = None
        self.entry_side = None
        self.entry_px = None
        self.entry_qty = None
        self.closing = False

    # ── reconciliation from the user channel ──
    def on_order_update(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        status = ev.get("status")
        if oid is None or oid != self.entry_oid:
            return
        if status in {"canceled", "filled", "rejected", "expired"}:
            log.info(
                "[%s] MOM order terminal: status=%s oid=%s", self.label, status, oid
            )
            if self.closing or status in {"canceled", "rejected", "expired"}:
                # Close finished, or entry never rested → flat again.
                self._reset_position()

    def on_fill(self, ev: dict) -> None:
        oid = ev.get("exchange_oid")
        if oid is None or oid != self.entry_oid:
            return
        log.info(
            "[%s] MOM FILL side=%s px=%s qty=%s oid=%s",
            self.label,
            self.entry_side,
            ev.get("px"),
            ev.get("qty"),
            oid,
        )
        if self.closing:
            self._reset_position()

    def cancel_all(self) -> None:
        """Engine calls this on window-expire / shutdown — pull any live oid."""
        if self.entry_oid is not None:
            try:
                self.router.cancel(
                    self.entry_oid, **self._attribution(self.entry_side or "?")
                )
            except Exception as e:  # noqa: BLE001
                log.warning("[%s] MOM cancel_all failed: %s", self.label, e)
        self._reset_position()

    # ── logging (the validation instrument) ──
    def _log_feed(
        self,
        now: float,
        bq: Optional[Quote],
        btr: Optional[Trade],
        yq: Optional[Quote],
        nq: Optional[Quote],
        signal: Optional[float],
        phase: str,
        action: str,
    ) -> None:
        if bq is None:
            binance = "binance:MISSING"
        else:
            age = (time.time_ns() - bq.received_ns) / 1e9
            binance = (
                f"binance:{self.bsym} bid={_f(bq.best_bid)} ask={_f(bq.best_ask)} "
                f"mid={_f(bq.mid)} age={age:.1f}s"
            )
        # Append the last executed Binance trade (price/size/taker side + age)
        # so the validation log shows the trade WS is live alongside the BBA.
        if btr is None:
            binance += " trade=—"
        else:
            t_age = (time.time_ns() - btr.received_ns) / 1e9
            binance += (
                f" trade={_f(btr.price)}x{_f(btr.size)} {btr.side} "
                f"age={t_age:.1f}s"
            )
        poly = (
            f"yes(b/a)={_f(_b(yq))}/{_f(_a(yq))} "
            f"no(b/a)={_f(_b(nq))}/{_f(_a(nq))}"
            if (yq is not None or nq is not None)
            else "poly:MISSING"
        )
        sig = "warming" if signal is None else f"{signal:+.1f}bps"
        if self.entry_side is None:
            pos = "none"
        else:
            pos = f"{self.entry_side}@{_f(self.entry_px)}x{_f(self.entry_qty)}"
            if self.closing:
                pos += "(closing)"
        log.info(
            "[%s] FEED %s | %s | signal=%s phase=%s pos=%s action=%s",
            self.label,
            binance,
            poly,
            sig,
            phase,
            pos,
            action,
        )


def _f(v: Optional[float]) -> str:
    return f"{v:.4f}" if isinstance(v, (int, float)) else "—"


def _b(q: Optional[Quote]) -> Optional[float]:
    return q.best_bid if q is not None else None


def _a(q: Optional[Quote]) -> Optional[float]:
    return q.best_ask if q is not None else None
