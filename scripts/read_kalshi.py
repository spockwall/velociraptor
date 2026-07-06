"""
Read Kalshi recorder MessagePack files into pandas DataFrames.

Kalshi files are organised by series, date, and window interval — one file per
window (no up/down split: the parser folds YES/NO into a single two-sided book,
so `mid` IS the implied probability of YES; NO is `1 - mid`):

    {base_path}/{series}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}.mpack
    ...and compressed variants ending in .mpack.zst

Record schema (snapshots only — the Kalshi feed carries no trade events):
    sequence:       u64
    ex_timestamp:   i64  — venue UTC ns (0 on the initial full snapshot)
    recv_timestamp: i64  — local receive UTC ns
    bids / asks:    [[price, qty], ...]  top-N, best first

Usage:
    pip install msgpack pandas zstandard

    # One window
    python scripts/read_kalshi.py data/kalshi/KXBTC15M/2026-07-06/03:30-03:45.mpack

    # All windows in a date directory
    python scripts/read_kalshi.py data/kalshi/KXBTC15M/2026-07-06/

    # All dates for a series
    python scripts/read_kalshi.py data/kalshi/KXBTC15M/

    # Per-window summary (record count, mean mid/spread, mean latency)
    python scripts/read_kalshi.py data/kalshi/KXBTC15M/2026-07-06/ --summary
"""

import argparse
import io
import struct
import sys
from pathlib import Path

import msgpack
import pandas as pd


# ── Low-level readers ────────────────────────────────────────────────────────


def _iter_records(buf):
    """
    Yield raw dicts from a readable binary stream (length-prefixed msgpack).
    A truncated final frame (recorder killed mid-write) is skipped with a
    warning instead of raising.
    """
    while header := buf.read(4):
        if len(header) < 4:
            print("warning: truncated length prefix at EOF — skipped", file=sys.stderr)
            return
        (n,) = struct.unpack("<I", header)
        payload = buf.read(n)
        if len(payload) < n:
            print(
                f"warning: truncated final record (wanted {n} bytes, got {len(payload)}) — skipped",
                file=sys.stderr,
            )
            return
        yield msgpack.unpackb(payload, raw=False)


def _read_file(path: Path) -> list:
    if path.suffix == ".zst":
        import zstandard

        dctx = zstandard.ZstdDecompressor()
        with open(path, "rb") as fh:
            with dctx.stream_reader(fh) as reader:
                return list(_iter_records(io.BufferedReader(reader)))
    with open(path, "rb") as f:
        return list(_iter_records(f))


def _to_df(records: list) -> pd.DataFrame:
    if not records:
        return pd.DataFrame()
    df = pd.DataFrame(records)
    # `recv_timestamp` (local receive ns) drives the human-facing `ts`;
    # `ex_timestamp` (venue ns, 0 on the initial full snapshot) is kept
    # alongside, surfaced as a venue→receive latency column.
    if "recv_timestamp" in df.columns:
        df.insert(0, "ts", pd.to_datetime(df["recv_timestamp"], unit="ns", utc=True))
        if "ex_timestamp" in df.columns:
            ex = df["ex_timestamp"].where(df["ex_timestamp"] != 0)
            df["latency_ms"] = (df["recv_timestamp"] - ex) / 1e6
    # Derive mid, spread, wmid from bids/asks. mid = implied P(YES).
    if "bids" in df.columns and "asks" in df.columns:
        best_bid = df["bids"].map(lambda x: x[0][0] if x else float("nan"))
        best_bid_qty = df["bids"].map(lambda x: x[0][1] if x else float("nan"))
        best_ask = df["asks"].map(lambda x: x[0][0] if x else float("nan"))
        best_ask_qty = df["asks"].map(lambda x: x[0][1] if x else float("nan"))
        df["mid"] = (best_bid + best_ask) / 2
        df["spread"] = best_ask - best_bid
        total_qty = best_bid_qty + best_ask_qty
        df["wmid"] = (best_ask * best_bid_qty + best_bid * best_ask_qty) / total_qty
        df["yes_prob"] = df["mid"]
        df["no_prob"] = 1 - df["mid"]
    return df


# ── File discovery ───────────────────────────────────────────────────────────


def _discover_files(root: Path) -> list[Path]:
    """
    Recursively find all .mpack / .mpack.zst files under root.
    When both exist for the same stem, keep only the .zst.
    """
    candidates: dict[str, Path] = {}
    for f in sorted(root.rglob("*.mpack")) + sorted(root.rglob("*.mpack.zst")):
        stem = f.name.removesuffix(".zst").removesuffix(".mpack")
        key = str(f.parent / stem)
        candidates[key] = f  # .zst sorts after .mpack → wins on duplicate
    return sorted(candidates.values())


def _window_from_path(path: Path) -> str:
    """Extract '03:30-03:45' from a filename like '03:30-03:45.mpack'."""
    return path.name.removesuffix(".zst").removesuffix(".mpack")


def _series_date_from_path(path: Path) -> tuple[str | None, str | None]:
    """Recover series/date from `{base}/{series}/{YYYY-MM-DD}/{window}.mpack[.zst]`."""
    parts = path.resolve().parts
    if len(parts) < 3:
        return None, None
    return parts[-3], parts[-2]


# ── Public loaders ───────────────────────────────────────────────────────────


def load(target: str) -> pd.DataFrame:
    """
    Load one file, a date directory, or a full series directory into a
    DataFrame. Adds 'series', 'date', and 'window' (HH:MM-HH:MM) columns
    from the path.
    """
    p = Path(target)
    files = [p] if p.is_file() else _discover_files(p)
    if not files:
        raise FileNotFoundError(f"No .mpack or .mpack.zst files found under {p}")

    frames = []
    for f in files:
        records = _read_file(f)
        if not records:
            continue
        df = _to_df(records)
        series, date = _series_date_from_path(f)
        df["series"] = series
        df["date"] = date
        df["window"] = _window_from_path(f)
        frames.append(df)

    return pd.concat(frames, ignore_index=True) if frames else pd.DataFrame()


def summary(target: str) -> pd.DataFrame:
    """Per-window summary: record count, first/last ts, mean mid/spread/latency."""
    df = load(target)
    if df.empty:
        return df

    rows = []
    for (date, window), g in df.groupby(["date", "window"]):
        rows.append(
            {
                "date": date,
                "window": window,
                "records": len(g),
                "first_ts": g["ts"].min(),
                "last_ts": g["ts"].max(),
                "mean_mid": round(g["mid"].mean(), 4) if "mid" in g.columns else None,
                "mean_spread": (
                    round(g["spread"].mean(), 4) if "spread" in g.columns else None
                ),
                "mean_latency_ms": (
                    round(g["latency_ms"].mean(), 1)
                    if "latency_ms" in g.columns
                    else None
                ),
            }
        )
    return pd.DataFrame(rows).sort_values(["date", "window"]).reset_index(drop=True)


# ── CLI ───────────────────────────────────────────────────────────────────────


def main():
    parser = argparse.ArgumentParser(
        description="Read Kalshi recorder .mpack files",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("target", help="File, date dir, or series dir to load")
    parser.add_argument(
        "--summary", action="store_true", help="Print per-window summary table"
    )
    parser.add_argument(
        "--head", type=int, default=20, help="Rows to display (default: 20)"
    )
    args = parser.parse_args()

    if args.summary:
        df = summary(args.target)
        print(df.to_string(index=False))
        return

    df = load(args.target)
    if df.empty:
        print("No records found.")
        return

    display_cols = [
        "ts", "window", "yes_prob", "no_prob", "spread", "wmid",
        "latency_ms", "sequence",
    ]
    display_cols = [c for c in display_cols if c in df.columns]
    print(df[display_cols].head(args.head).to_string(index=False))

    windows = df["window"].nunique() if "window" in df.columns else 0
    print(f"\n{len(df):,} records  |  {windows} window(s)")


if __name__ == "__main__":
    main()
