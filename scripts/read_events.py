"""Load archived event CSV files into a pandas DataFrame.

Two streams:

  - **Server recorder** writes CSVs under `./data/...`:
        {base}/events/{YYYY-MM-DD}.csv                     — all UserEvent variants
        {base}/{exchange}/{symbol}/{YYYY-MM-DD}.csv        — orderbook snapshots
        {base}/{exchange}/{symbol}/{YYYY-MM-DD}-trades.csv — last-trade events

  - **Trading engine** writes CSVs under `./data/engine_log/`:
        {engine_base}/actions/{YYYY-MM-DD}.csv  — orders the engine initiated
        {engine_base}/events/{YYYY-MM-DD}.csv   — events the engine received

Both are plain CSV — `pd.read_csv(path)` works directly. This script is
just convenient sugar for resolving the date/path.

Usage:

    python scripts/read_events.py events                    # today, server recorder
    python scripts/read_events.py actions --base ./data/engine_log
    python scripts/read_events.py events  --date 2026-05-12
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
from pathlib import Path

import pandas as pd


def load_csv(
    kind: str,
    date: str | None = None,
    base: str | Path = "./data",
) -> pd.DataFrame:
    """Load one day's CSV by `kind`.

    Server-recorder kinds: `events`.
    Engine-log kinds:      `actions`, `events` (pass `base=./data/engine_log`).
    """
    if date is None:
        date = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d")
    path = Path(base) / kind / f"{date}.csv"
    if not path.exists():
        raise FileNotFoundError(f"no CSV at {path}")
    df = pd.read_csv(path)
    if "ts_ns" in df.columns and not df.empty:
        df.insert(0, "ts", pd.to_datetime(df["ts_ns"], unit="ns", utc=True))
    return df


def _cli() -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("kind", help="Subfolder name under base (e.g. events, actions)")
    p.add_argument("--date", help="YYYY-MM-DD (default: today UTC)")
    p.add_argument(
        "--base",
        default="./data",
        help="Root path. Use `./data` for the server recorder, `./data/engine_log` for engine logs.",
    )
    p.add_argument("--head", type=int, default=20, help="Rows to print (0 = all)")
    args = p.parse_args()

    df = load_csv(args.kind, date=args.date, base=args.base)
    print(f"{len(df)} record(s) in {args.kind}", file=sys.stderr)
    pd.set_option("display.width", 200)
    pd.set_option("display.max_columns", 30)
    if args.head and len(df) > args.head:
        print(df.head(args.head))
    else:
        print(df)
    return 0


if __name__ == "__main__":
    raise SystemExit(_cli())
