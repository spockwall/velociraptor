"""CLI argument parser. Kept separate from `app.py` so the orchestration
class isn't intermingled with argparse boilerplate."""

from __future__ import annotations

import argparse
import logging

from .trading import available_strategies

log = logging.getLogger(__name__)

# Legacy `--step` → strategy name. Step 0 = observe.
_STEP_ALIASES = {0: "observe", 1: "probe", 2: "fill_once"}


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="python -m scripts.trading_engine",
        description=(
            "Velociraptor Python trading engine — single-strategy, "
            "event-driven. Launch one process per strategy/market."
        ),
    )

    # ── Strategy selection ──
    p.add_argument(
        "--strategy",
        choices=available_strategies(),
        default=None,
        help=(
            "What the engine runs. `observe` is the no-orders "
            "multi-exchange watcher. Other choices are per-Polymarket-"
            "window strategies registered in `trading/strategies/`."
        ),
    )
    p.add_argument(
        "--step",
        type=int,
        choices=[0, 1, 2],
        default=None,
        help="Deprecated. Use --strategy. 0→observe, 1→probe, 2→fill_once.",
    )

    # ── Markets to track / trade ──
    # Window strategies require exactly one --base-slugs value.
    # Observer accepts the full list and watches all of them.
    p.add_argument(
        "--base-slugs",
        nargs="+",
        default=["btc-updown-15m", "eth-updown-15m"],
        help=(
            "Polymarket base slugs. Window strategies (probe/fill_once/"
            "one_shot/momentum) require exactly one value. Observer "
            "watches all."
        ),
    )
    p.add_argument(
        "--kalshi-series",
        nargs="+",
        default=["KXBTC15M", "KXETH15M"],
        help="Kalshi series (observer only)",
    )
    p.add_argument(
        "--binance-symbols",
        nargs="+",
        default=["btcusdt", "ethusdt"],
        help="Binance USDT-margined futures symbols (observer only)",
    )
    p.add_argument(
        "--binance-spot-symbols",
        nargs="+",
        default=["btcusdt", "ethusdt"],
        help="Binance Spot symbols (observer only)",
    )

    # ── Endpoints ──
    p.add_argument("--backend-url", default="http://127.0.0.1:3000")
    p.add_argument("--market-pub", default="tcp://127.0.0.1:5555")
    p.add_argument(
        "--market-router",
        default="tcp://127.0.0.1:5556",
        help="zmq_server control-plane ROUTER for snapshot/bba subscribe handshake",
    )
    p.add_argument("--user-pub", default="tcp://127.0.0.1:5559")
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

    p.add_argument("--log-level", default="info")

    # ── Engine event log (durable action + event record) ──
    # Mandatory. The log is the only durable record of what the engine
    # placed / cancelled and what events it saw, so the engine refuses
    # to start if the directory isn't writable.
    p.add_argument(
        "--engine-log-dir",
        default="/syslog/trading_engine/",
        help=(
            "Directory for the engine's append-only action + event log. "
            "Daily-rotated CSV: {dir}/actions/<YYYY-MM-DD>.csv + "
            "{dir}/events/<YYYY-MM-DD>.csv. The directory is created if "
            "missing; the engine exits at startup if it can't be written."
        ),
    )

    args = p.parse_args()

    # Resolve --strategy from --step if needed, then drop --step.
    if args.strategy is None:
        if args.step is None:
            args.strategy = "probe"
        else:
            args.strategy = _STEP_ALIASES[args.step]
            log.warning(f"--step is deprecated; use --strategy {args.strategy}")
    elif args.step is not None:
        log.warning("--step ignored because --strategy was set")
    delattr(args, "step")
    return args
