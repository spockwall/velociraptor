"""
Read recorder MessagePack snapshot files into a pandas DataFrame.

Supports both raw (.mpack) and zstd-compressed (.mpack.zst) files.

Record schema (see docs/storage.md):
    sequence: u64
    ts_ns:    i64  — UTC nanoseconds since Unix epoch
    bids:     [[price, qty], ...]  top-N, best first
    asks:     [[price, qty], ...]  top-N, best first

File format: [u32 LE length][msgpack bytes] repeated.

Standard layout:
    {base_path}/{exchange}/{SYMBOL}/{YYYY-MM-DD}.mpack[.zst]

Usage:
    pip install msgpack pandas zstandard numpy
    python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
    python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack.zst
    python scripts/read_mpack.py data/binance/BTCUSDT/   # all files in directory
"""

import io
import struct
import sys
from pathlib import Path

import msgpack
import numpy as np
import pandas as pd


def _iter_records(buf):
    """Yield raw dicts from any readable binary stream."""
    while header := buf.read(4):
        (n,) = struct.unpack("<I", header)
        yield msgpack.unpackb(buf.read(n), raw=False)


def read_mpack(path: Path) -> pd.DataFrame:
    with open(path, "rb") as f:
        records = list(_iter_records(f))
    return _to_dataframe(records, path)


def read_mpack_zst(path: Path) -> pd.DataFrame:
    import zstandard
    dctx = zstandard.ZstdDecompressor()
    with open(path, "rb") as fh:
        with dctx.stream_reader(fh) as reader:
            records = list(_iter_records(io.BufferedReader(reader)))
    return _to_dataframe(records, path)


def _exchange_symbol_from_path(path: Path) -> tuple[str | None, str | None]:
    """Recover exchange/symbol from `{base}/{exchange}/{SYMBOL}/{date}.mpack[.zst]`."""
    parts = path.resolve().parts
    if len(parts) < 3:
        return None, None
    return parts[-3], parts[-2]


def _derive_metrics(df: pd.DataFrame) -> pd.DataFrame:
    """Add best_bid_px, best_ask_px, spread, mid, wmid from bids/asks."""
    if "bids" not in df.columns or "asks" not in df.columns:
        return df

    def _top(col):
        # Each row: list of [px, qty] pairs; take the first (best) entry.
        px = np.full(len(df), np.nan)
        qty = np.full(len(df), np.nan)
        for i, level in enumerate(df[col].values):
            if level:
                px[i], qty[i] = level[0][0], level[0][1]
        return px, qty

    best_bid_px, best_bid_qty = _top("bids")
    best_ask_px, best_ask_qty = _top("asks")

    df["best_bid_px"] = best_bid_px
    df["best_ask_px"] = best_ask_px
    df["spread"] = best_ask_px - best_bid_px
    df["mid"] = (best_bid_px + best_ask_px) / 2.0

    qty_sum = best_bid_qty + best_ask_qty
    with np.errstate(invalid="ignore", divide="ignore"):
        df["wmid"] = np.where(
            qty_sum > 0,
            (best_ask_px * best_bid_qty + best_bid_px * best_ask_qty) / qty_sum,
            np.nan,
        )
    return df


def _to_dataframe(records: list, path: Path) -> pd.DataFrame:
    df = pd.DataFrame(records)
    if df.empty:
        return df
    if "ts_ns" in df.columns:
        df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))

    exchange, symbol = _exchange_symbol_from_path(path)
    if exchange is not None:
        df["exchange"] = exchange
    if symbol is not None:
        df["symbol"] = symbol

    return _derive_metrics(df)


def load(target: str) -> pd.DataFrame:
    p = Path(target)
    if p.is_file():
        return read_mpack_zst(p) if p.suffix == ".zst" else read_mpack(p)

    files = sorted(p.glob("*.mpack")) + sorted(p.glob("*.mpack.zst"))
    if not files:
        raise FileNotFoundError(f"No .mpack or .mpack.zst files in {p}")

    # Prefer .mpack.zst over .mpack when both exist for the same date
    seen: dict[str, Path] = {}
    for f in files:
        stem = f.name.removesuffix(".zst").removesuffix(".mpack")
        seen[stem] = f  # .zst sorts after .mpack, wins

    return pd.concat(
        (read_mpack_zst(f) if f.suffix == ".zst" else read_mpack(f)
         for f in sorted(seen.values())),
        ignore_index=True,
    )


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "data"
    df = load(target)
    cols = [c for c in ["ts", "exchange", "symbol", "sequence",
                        "best_bid_px", "best_ask_px", "spread", "mid", "wmid"]
            if c in df.columns]
    print(df[cols].head(20))
    print(f"\n{len(df)} records loaded")
