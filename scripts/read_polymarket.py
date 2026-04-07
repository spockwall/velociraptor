"""
Read Polymarket recorder MessagePack files into pandas DataFrames.

Polymarket files are organised by base slug, date, and window interval:

    {base_path}/polymarket/{slug}/{YYYY-MM-DD}/{HH:MM}-{HH:MM}-up.mpack
                                              /{HH:MM}-{HH:MM}-down.mpack
    ...and compressed variants ending in .mpack.zst

Usage:
    pip install msgpack pandas zstandard

    # One window side
    python scripts/read_polymarket.py /tmp/polymarket_data/polymarket/btc-updown-5m/2026-04-06/12:00-12:05-down.mpack

    # All files under a date directory (both sides, all windows)
    python scripts/read_polymarket.py /tmp/polymarket_data/polymarket/btc-updown-5m/2026-04-06/

    # All dates for a slug
    python scripts/read_polymarket.py /tmp/polymarket_data/polymarket/btc-updown-5m/

    # Per-window summary (record count, mean mid, mean spread)
    python scripts/read_polymarket.py /tmp/polymarket_data/polymarket/btc-updown-5m/2026-04-06/ --summary

    # Merge up and down into aligned rows
    python scripts/read_polymarket.py /tmp/polymarket_data/polymarket/btc-updown-5m/2026-04-06/ --merge
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
    """Yield raw dicts from a readable binary stream (length-prefixed msgpack)."""
    while header := buf.read(4):
        (n,) = struct.unpack("<I", header)
        yield msgpack.unpackb(buf.read(n), raw=False)


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
    if "ts_ns" in df.columns:
        df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    # Derive mid, spread, wmid from bids/asks if present.
    if "bids" in df.columns and "asks" in df.columns:
        best_bid = df["bids"].map(lambda x: x[0][0] if x else float("nan"))
        best_bid_qty = df["bids"].map(lambda x: x[0][1] if x else float("nan"))
        best_ask = df["asks"].map(lambda x: x[0][0] if x else float("nan"))
        best_ask_qty = df["asks"].map(lambda x: x[0][1] if x else float("nan"))
        df["mid"] = (best_bid + best_ask) / 2
        df["spread"] = best_ask - best_bid
        total_qty = best_bid_qty + best_ask_qty
        df["wmid"] = (best_ask * best_bid_qty + best_bid * best_ask_qty) / total_qty
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


def _side_from_path(path: Path) -> str:
    """Extract 'up' or 'down' from a filename like '06:15-06:20-up.mpack'."""
    stem = path.name.removesuffix(".zst").removesuffix(".mpack")
    parts = stem.rsplit("-", 1)
    return parts[-1] if len(parts) == 2 else "unknown"


def _window_from_path(path: Path) -> str:
    """Extract '06:15-06:20' from a filename like '06:15-06:20-up.mpack'."""
    stem = path.name.removesuffix(".zst").removesuffix(".mpack")
    parts = stem.rsplit("-", 1)
    return parts[0] if len(parts) == 2 else stem


# ── Public loaders ───────────────────────────────────────────────────────────


def load(target: str) -> pd.DataFrame:
    """
    Load one file, a date directory, or a full slug directory into a DataFrame.
    Adds 'side' (up/down) and 'window' (HH:MM-HH:MM) columns from filenames.
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
        df["side"] = _side_from_path(f)
        df["window"] = _window_from_path(f)
        frames.append(df)

    return pd.concat(frames, ignore_index=True) if frames else pd.DataFrame()


def load_side(target: str, side: str) -> pd.DataFrame:
    """Load only 'up' or 'down' files from a directory."""
    df = load(target)
    if df.empty:
        return df
    return df[df["side"] == side].reset_index(drop=True)


def merge_sides(target: str, tolerance_ms: int = 500) -> pd.DataFrame:
    """
    Load up and down files and align them on nearest timestamp.
    Returns one row per up-snapshot with paired down columns (_up / _down suffixes).
    """
    df = load(target)
    if df.empty:
        return df

    up = (
        df[df["side"] == "up"]
        .drop(columns=["side"])
        .sort_values("ts")
        .reset_index(drop=True)
    )
    down = (
        df[df["side"] == "down"]
        .drop(columns=["side"])
        .sort_values("ts")
        .reset_index(drop=True)
    )

    merged = pd.merge_asof(
        up,
        down,
        on="ts",
        tolerance=pd.Timedelta(milliseconds=tolerance_ms),
        direction="nearest",
        suffixes=("_up", "_down"),
    )
    if "mid_up" in merged.columns and "mid_down" in merged.columns:
        merged["sum_mid"] = merged["mid_up"].fillna(0) + merged["mid_down"].fillna(0)
    return merged


def summary(target: str) -> pd.DataFrame:
    """Per-window summary: record count, first/last ts, mean mid and spread per side."""
    df = load(target)
    if df.empty:
        return df

    rows = []
    for (window, side), g in df.groupby(["window", "side"]):
        rows.append(
            {
                "window": window,
                "side": side,
                "records": len(g),
                "first_ts": g["ts"].min(),
                "last_ts": g["ts"].max(),
                "mean_mid": round(g["mid"].mean(), 4) if "mid" in g.columns else None,
                "mean_spread": (
                    round(g["spread"].mean(), 6) if "spread" in g.columns else None
                ),
            }
        )
    return pd.DataFrame(rows).sort_values(["window", "side"]).reset_index(drop=True)


# ── CLI ───────────────────────────────────────────────────────────────────────


def main():
    parser = argparse.ArgumentParser(
        description="Read Polymarket recorder .mpack files",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("target", help="File, date dir, or slug dir to load")
    parser.add_argument(
        "--side", choices=["up", "down"], help="Filter to one side only"
    )
    parser.add_argument(
        "--summary", action="store_true", help="Print per-window summary table"
    )
    parser.add_argument(
        "--merge", action="store_true", help="Merge up/down on nearest timestamp"
    )
    parser.add_argument(
        "--head", type=int, default=20, help="Rows to display (default: 20)"
    )
    args = parser.parse_args()

    if args.summary:
        df = summary(args.target)
        print(df.to_string(index=False))
        return

    if args.merge:
        df = merge_sides(args.target)
    elif args.side:
        df = load_side(args.target, args.side)
    else:
        df = load(args.target)

    if df.empty:
        print("No records found.")
        return

    display_cols = ["ts", "window", "side", "mid", "spread", "wmid", "sequence"]
    display_cols = [c for c in display_cols if c in df.columns]
    print(df[display_cols].head(args.head).to_string(index=False))

    side_counts = df["side"].value_counts().to_dict() if "side" in df.columns else {}
    print(f"\n{len(df):,} records  |  {side_counts}")


if __name__ == "__main__":
    main()
