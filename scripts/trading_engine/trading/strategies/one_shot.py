"""One-shot strategy — place one resting limit order per window, then park.

Built to *watch the orderbook change*: unlike `fill_once` (which fires a
market order that fills immediately and leaves no resting state), this places a
single **limit buy at the current best bid** so the order actually rests in the
book. You can then see the bid-side qty at that price grow by your size — the
latency tracer's price/qty check confirms it as `matched=1`.

Behaviour (mirrors `fill_once`'s window/guard structure):

  - On the first quote per window that passes the safe-mid guard, place a buy
    limit at `best_bid` (joins the touch — visible, but does NOT cross the
    spread, so no accidental fill).
  - Latch `_sent_this_window`; a rollover clears it for the next window.
  - The order is left RESTING so you can observe the book. It is cancelled on
    engine shutdown (`teardown`) for a clean exit.

Resting at `best_bid` means zero fill risk in a normal book (every resting ask
is strictly higher). If you want to rest deeper, pass `--limit-offset-ticks N`
to sit N ticks below the best bid (a fresh, easy-to-spot bid level).
"""

from __future__ import annotations

import logging

from ...market import Quote
from ..dispatcher import Dispatcher
from .base import Strategy
from ..helpers import (
    PIN_PX,
    SAFE_MID_HIGH,
    SAFE_MID_LOW,
    TICK,
    clamp_px,
    qty_for_notional,
    safe_mid_guard,
)

log = logging.getLogger(__name__)


class OneShotStrategy(Strategy):
    name = "one_shot"
    is_window = True  # built with a PolymarketWindow by Strategy.build

    @classmethod
    def build(cls, args, *, market, router, state):
        # Window kwargs + this strategy's extra knobs (order size + how far
        # below the touch to rest).
        kwargs = cls.window_kwargs(args, market=market, router=router, state=state)
        kwargs["order_notional_usd"] = args.order_notional_usd
        kwargs["limit_offset_ticks"] = getattr(args, "limit_offset_ticks", 0)
        return cls(**kwargs)

    def __init__(
        self,
        *,
        order_notional_usd: float = 10.0,
        limit_offset_ticks: int = 0,
        pin_px: float = PIN_PX,
        safe_mid_low: float = SAFE_MID_LOW,
        safe_mid_high: float = SAFE_MID_HIGH,
        **kwargs,
    ):
        super().__init__(**kwargs)
        if self.window is None:
            raise ValueError("OneShotStrategy needs a Polymarket window")
        self.order_notional_usd = order_notional_usd
        # How many ticks BELOW best bid to rest (0 = join the best bid).
        self.limit_offset_ticks = max(0, int(limit_offset_ticks))
        self.pin_px = pin_px
        self.safe_mid_low = safe_mid_low
        self.safe_mid_high = safe_mid_high
        # Window-scoped state.
        self._sent_this_window: bool = False
        # Order left resting so we can observe the book; cancelled on teardown.
        self._resting_oid: str | None = None

    def required_topics(self) -> list[tuple[str, str, str]]:
        return [("polymarket", self.window.base_slug, "snapshot")]

    def setup(self, dispatcher: Dispatcher) -> None:
        # Fire on the first valid quote (no default throttle).
        dispatcher.register_quote(
            "polymarket", self.window.base_slug, self._on_quote, min_interval_ms=0
        )
        dispatcher.register_rollover(
            "polymarket", self.window.base_slug, self._on_rollover
        )
        dispatcher.register_bootstrap(
            "polymarket", self.window.base_slug, self._on_bootstrap
        )

    # ── handlers ──

    def _on_bootstrap(self, full_slug: str) -> None:
        # No-op. MarketState.on_bootstrap already populated asset_ids; the
        # engine pre-stamped self.window.full_slug.
        pass

    def _on_rollover(self, full_slug: str) -> None:
        # asset_ids already resolved by MarketState.on_rollover. New window =
        # we may place again. The previous window's order (if any) stays as
        # `_resting_oid` and is cancelled on teardown.
        prev_full = self.window.full_slug
        self.window.full_slug = full_slug
        self._sent_this_window = False
        log.info(f"[{self.label}] rollover {prev_full!r} → {full_slug!r}")

    def _on_quote(self, q: Quote) -> None:
        if self._sent_this_window:
            return
        asset_id = self._asset_id("up")
        if asset_id is None:
            log.debug(f"[{self.label}] no UP asset_id resolved yet; skip")
            return

        reason = safe_mid_guard(
            self.market,
            "polymarket",
            self.window.base_slug,
            low=self.safe_mid_low,
            high=self.safe_mid_high,
            pin_px=self.pin_px,
        )
        if reason is not None:
            log.debug(f"[{self.label}] skip: safe_mid_guard={reason}")
            return

        # Need a best bid to rest on. `q.best_bid` is the bid price (the BBA
        # quote carries price only; that's all we need for the resting price).
        if q.best_bid is None:
            log.debug(f"[{self.label}] skip: no best_bid in latest quote")
            return

        # Rest at best_bid (offset ticks below it if requested). clamp_px keeps
        # it on the tick grid and inside [MIN_PX, MAX_PX].
        target_px = clamp_px(q.best_bid - self.limit_offset_ticks * TICK)
        target_qty = qty_for_notional(self.order_notional_usd, target_px)
        if target_qty <= 0:
            log.debug(f"[{self.label}] skip: target_qty <= 0 (px={target_px})")
            return

        slug = self.window.full_slug or self.window.base_slug
        # Deterministic per (window, asset_id) so a re-fire hits the executor's
        # idempotency cache instead of double-placing.
        client_oid = f"te-1shot-{slug[:24]}-{asset_id[:10]}"
        # Latch BEFORE the call: one window = one attempt, success or not.
        self._sent_this_window = True
        log.info(
            f"[{self.label}] ONE_SHOT place_limit BUY px={target_px:.2f} "
            f"qty={target_qty:.2f} (rest @ best_bid"
            f"{f'-{self.limit_offset_ticks}t' if self.limit_offset_ticks else ''}) "
            f"client_oid={client_oid}"
        )
        try:
            # GTC so it RESTS in the book (the whole point — we want to watch
            # the bid side change). The tracer confirms it via price/qty.
            ack = self.router.place_limit(
                symbol=asset_id,
                side="buy",
                px=target_px,
                qty=target_qty,
                client_oid=client_oid,
                tif="GTC",
                quote=q,
                **self._attribution(),
            )
        except Exception as e:  # noqa: BLE001
            log.warning(f"[{self.label}] ONE_SHOT place_limit failed: {e}")
            return

        oid = ack.get("exchange_oid")
        log.info(f"[{self.label}] ONE_SHOT resting oid={oid} — watch the book")
        if oid is not None:
            self._resting_oid = oid

    # ── teardown ──

    def teardown(self) -> None:
        # Cancel the resting order on clean exit so we don't leave a live
        # order on the book after the engine stops.
        if self._resting_oid is not None:
            try:
                self.router.cancel(self._resting_oid, **self._attribution())
                log.info(f"[{self.label}] ONE_SHOT teardown cancel oid={self._resting_oid}")
            except Exception as e:  # noqa: BLE001
                log.warning(f"[{self.label}] ONE_SHOT teardown cancel failed: {e}")
            self._resting_oid = None
