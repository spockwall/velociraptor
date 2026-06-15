#!/usr/bin/env python3
"""Summarize a latency-trace CSV produced by the trading engine
(`--latency-csv`). Prints p50 / p90 / p99 / max (and count) per pipeline stage,
so you can see at a glance where the end-to-end order latency actually goes.

Usage:
    python scripts/analyze_latency.py data/latency.csv
    python scripts/analyze_latency.py data/latency.csv --side buy --kind market

The CSV's derived `*_ms` columns are the per-stage deltas (see
scripts/trading_engine/utils/latency.py for the timestamp chain). This script
only reads those columns, so it has no dependency on the engine.

Stage guide (what each number means, and whether clocks are trustworthy):
    md_wire_ms       venue → orderbook_server  (CROSS-CLOCK: needs NTP; can be -)
    md_transport_ms  orderbook_server → engine
    eng_to_exec_ms   engine → executor          (CROSS-CLOCK)
    exec_overhead_ms executor decode/queue      (single clock)
    venue_rtt_ms     the exchange REST round-trip (single clock) ← key order cost
    resp_return_ms   executor → engine return
    order_rtt_ms     full engine-side order round-trip (decide → response)
    bba_match_ms     decide → our order shows in the book (FAST signal)
    fill_evt_ms      decide → venue user-channel fill (SLOW signal)
"""

from __future__ import annotations

import argparse
import csv
import math
import sys
from typing import Optional

# The derived per-stage columns, in pipeline order, with one-line labels.
STAGES = [
    ("md_wire_ms", "venue→server wire (cross-clock)"),
    ("md_transport_ms", "server→engine transport"),
    ("eng_to_exec_ms", "engine→executor (cross-clock)"),
    ("exec_overhead_ms", "executor decode/queue"),
    ("venue_rtt_ms", "exchange REST round-trip"),
    ("resp_return_ms", "executor→engine return"),
    ("order_rtt_ms", "FULL order round-trip (decide→resp)"),
    ("bba_match_ms", "decide→next-book-frame (timing only)"),
    ("book_match_ms", "decide→order APPEARS in book (px/qty)"),
    ("book_fill_ms", "decide→order CONSUMED on book (matched)"),
    ("fill_evt_ms", "decide→user-fill (authoritative)"),
]


def _pct(sorted_vals: list[float], p: float) -> float:
    """Nearest-rank percentile (p in [0,100]) of a pre-sorted list."""
    if not sorted_vals:
        return math.nan
    k = max(
        0, min(len(sorted_vals) - 1, int(math.ceil(p / 100.0 * len(sorted_vals))) - 1)
    )
    return sorted_vals[k]


def _fmt(v: float) -> str:
    return "—" if math.isnan(v) else f"{v:8.3f}"


def main(argv: Optional[list[str]] = None) -> int:
    p = argparse.ArgumentParser(
        prog="analyze_latency.py",
        description="Per-stage p50/p90/p99 latency from an engine latency-trace CSV.",
    )
    p.add_argument("csv_path", help="latency-trace CSV (from --latency-csv).")
    p.add_argument("--side", choices=["buy", "sell"], help="filter to one side.")
    p.add_argument(
        "--kind", choices=["limit", "market"], help="filter to one order kind."
    )
    p.add_argument("--symbol", help="filter to one symbol (asset id).")
    p.add_argument(
        "--only-ok",
        action="store_true",
        help="exclude rows where the place failed (ok=0).",
    )
    args = p.parse_args(argv)

    try:
        with open(args.csv_path, newline="") as fh:
            reader = csv.DictReader(fh)
            header = reader.fieldnames or []
            rows = list(reader)
    except FileNotFoundError:
        print(f"no such file: {args.csv_path}", file=sys.stderr)
        return 1

    # Stale-header guard: a CSV written by an older tracer schema has different
    # columns, so DictReader would mis-map values onto the wrong names and the
    # report would be garbage (raw ns printed as ms, etc.). Refuse loudly.
    missing = [c for c, _ in STAGES if c not in header]
    if missing:
        print(
            f"CSV schema mismatch: {args.csv_path} is missing columns {missing}.\n"
            f"It was likely written by an older tracer version. Archive it and "
            f"start a fresh --latency-csv run (the new run writes the current "
            f"header).",
            file=sys.stderr,
        )
        return 2

    # Apply filters.
    def keep(r: dict) -> bool:
        if args.side and r.get("side") != args.side:
            return False
        if args.kind and r.get("kind") != args.kind:
            return False
        if args.symbol and r.get("symbol") != args.symbol:
            return False
        if args.only_ok and r.get("ok") not in ("1", "True", "true"):
            return False
        return True

    rows = [r for r in rows if keep(r)]
    if not rows:
        print("no rows after filtering.", file=sys.stderr)
        return 1

    print(f"\nlatency report — {len(rows)} orders  ({args.csv_path})")
    filt = [f for f in (args.side, args.kind, args.symbol) if f]
    if filt:
        print(f"filters: {', '.join(filt)}{' only-ok' if args.only_ok else ''}")

    # Two book observations:
    #   appeared  — our order showed up in the book (qty up at our price)
    #   consumed  — our resting qty was eaten (qty down) = matched/hit
    appeared = sum(1 for r in rows if r.get("book_confirmed") == "1")
    unseen = sum(1 for r in rows if r.get("book_confirmed") == "0")
    consumed = sum(1 for r in rows if r.get("book_filled") == "1")
    gone = sum(1 for r in rows if r.get("book_fully_gone") == "1")
    filled_evt = sum(1 for r in rows if (r.get("t_fill_evt_ns") or "0") != "0")
    print()
    print(
        f"appeared in book (px/qty): {appeared}/{len(rows)}  (never-seen: {unseen})"
    )
    print(
        f"consumed on book (matched/hit): {consumed}  "
        f"(fully gone: {gone}) · user-fill events: {filled_evt}"
    )
    # Per-order detail so you can eyeball appeared vs consumed.
    for r in rows:
        if r.get("book_confirmed") == "1":
            line = (
                f"  ✓ {r.get('side'):4s} {r.get('kind'):6s} "
                f"px={r.get('px')} qty={r.get('qty')} → "
                f"appeared +{r.get('match_dqty')} @ {r.get('match_px')} "
                f"({r.get('book_match_ms')} ms)"
            )
            if r.get("book_filled") == "1":
                tag = "FULLY MATCHED" if r.get("book_fully_gone") == "1" else "partial"
                line += (
                    f"  → CONSUMED {r.get('book_fill_dqty')} "
                    f"({r.get('book_fill_ms')} ms) [{tag}]"
                )
            else:
                line += "  → rested unmatched"
            print(line)
    print()
    print(f"{'stage':38s} {'n':>5s} {'p50':>9s} {'p90':>9s} {'p99':>9s} {'max':>9s}")
    print("-" * 84)

    for col, label in STAGES:
        vals = []
        for r in rows:
            cell = r.get(col, "")
            if cell == "" or cell is None:
                continue
            try:
                vals.append(float(cell))
            except ValueError:
                continue
        vals.sort()
        n = len(vals)
        if n == 0:
            print(f"{label:38s} {0:>5d} {'—':>9s} {'—':>9s} {'—':>9s} {'—':>9s}")
            continue
        print(
            f"{label:38s} {n:>5d} "
            f"{_fmt(_pct(vals, 50))} {_fmt(_pct(vals, 90))} "
            f"{_fmt(_pct(vals, 99))} {_fmt(vals[-1])}"
        )

    print("-" * 84)
    print("all values in milliseconds. cross-clock stages need NTP to trust;")
    print("negative values there mean the two machines' clocks disagree.\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
