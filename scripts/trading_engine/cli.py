"""CLI argument parser. Kept separate from `app.py` so the orchestration
class isn't intermingled with argparse boilerplate."""

from __future__ import annotations

import argparse
import logging

from .trading import available_strategies

log = logging.getLogger(__name__)

# Legacy `--step` → strategy name. Step 0 is the observer (not a Strategy).
_STEP_ALIASES = {0: "observe", 1: "probe", 2: "fill_once"}


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="python -m scripts.trading_engine",
        description="Velociraptor Python trading engine — Polymarket up/down windows.",
    )
    # New selector: --strategy.
    strategy_choices = ["observe", *available_strategies()]
    p.add_argument(
        "--strategy",
        choices=strategy_choices,
        default=None,
        help=(
            "What the engine runs. "
            "`observe` is the no-orders multi-exchange watcher. "
            "Other choices are per-window strategies registered in "
            "`trading/strategies/`."
        ),
    )
    # Legacy: --step. Mapped to --strategy via _STEP_ALIASES.
    p.add_argument(
        "--step",
        type=int,
        choices=[0, 1, 2],
        default=None,
        help=(
            "Deprecated. Use --strategy. "
            "0→observe, 1→probe, 2→fill_once."
        ),
    )

    # ── Markets to track / trade ──
    p.add_argument(
        "--base-slugs",
        nargs="+",
        default=["btc-updown-15m", "eth-updown-15m"],
        help="Polymarket base slugs to track / trade",
    )
    p.add_argument(
        "--kalshi-series",
        nargs="+",
        default=["KXBTC15M", "KXETH15M"],
        help="Kalshi series to track (step 0 only)",
    )
    p.add_argument(
        "--binance-symbols",
        nargs="+",
        default=["btcusdt", "ethusdt"],
        help="Binance USDT-margined futures symbols to track (step 0 only)",
    )
    p.add_argument(
        "--binance-spot-symbols",
        nargs="+",
        default=["btcusdt", "ethusdt"],
        help="Binance Spot symbols to track (step 0 only)",
    )

    # ── Endpoints ──
    p.add_argument("--backend-url", default="http://127.0.0.1:3000")
    p.add_argument("--market-pub", default="tcp://127.0.0.1:5555")
    p.add_argument(
        "--market-router",
        default="tcp://127.0.0.1:5556",
        help="zmq_server control-plane ROUTER for snapshot/bba subscribe handshake",
    )
    p.add_argument("--user-pub", default="ipc:///tmp/trading/ws_status.sock")
    p.add_argument("--router-endpoint", default="tcp://127.0.0.1:5557")

    # ── Strategy params ──
    p.add_argument("--order-notional-usd", type=float, default=10.0)
    p.add_argument(
        "--safe-mid-low",
        type=float,
        default=0.30,
        help="Skip placing on a side whose mid is below this. Default 0.30.",
    )
    p.add_argument(
        "--safe-mid-high",
        type=float,
        default=0.70,
        help="Skip placing on a side whose mid is above this. Default 0.70.",
    )

    # ── Loop cadence ──
    p.add_argument("--tick-secs", type=float, default=2.0)
    p.add_argument("--rediscover-secs", type=float, default=30.0)
    p.add_argument(
        "--report-secs",
        type=float,
        default=5.0,
        help="Step 0: how often to dump the orderbook status table",
    )

    p.add_argument("--log-level", default="info")
    args = p.parse_args()

    # Resolve --strategy from --step if needed, then drop --step.
    if args.strategy is None:
        if args.step is None:
            args.strategy = "probe"  # historical default
        else:
            args.strategy = _STEP_ALIASES[args.step]
            log.warning(
                "--step is deprecated; use --strategy %s", args.strategy,
            )
    elif args.step is not None:
        log.warning("--step ignored because --strategy was set")
    delattr(args, "step")
    return args
