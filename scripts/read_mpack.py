"""
Read recorder MessagePack snapshot files into a pandas DataFrame.

Supports both raw (.mpack) and zstd-compressed (.mpack.zst) files.

File format: [u32 LE length][msgpack bytes] repeated.
File layout:  {base_path}/{exchange}/{symbol}/{YYYY-MM-DD}.mpack
              {base_path}/{exchange}/{symbol}/{YYYY-MM-DD}.mpack.zst

Usage:
    pip install msgpack pandas zstandard
    python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
    python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack.zst
    python scripts/read_mpack.py data/binance/BTCUSDT/   # all files in directory
"""

import io
import struct
import sys
from pathlib import Path

import msgpack
import pandas as pd


def _iter_records(buf: io.RawIOBase | io.BufferedIOBase):
    """Yield raw dicts from any readable binary stream."""
    while header := buf.read(4):
        (n,) = struct.unpack("<I", header)
        yield msgpack.unpackb(buf.read(n), raw=False)


def read_mpack(path: Path) -> pd.DataFrame:
    with open(path, "rb") as f:
        records = list(_iter_records(f))
    return _to_dataframe(records)


def read_mpack_zst(path: Path) -> pd.DataFrame:
    import zstandard
    dctx = zstandard.ZstdDecompressor()
    with open(path, "rb") as fh:
        with dctx.stream_reader(fh) as reader:
            records = list(_iter_records(io.BufferedReader(reader)))
    return _to_dataframe(records)


def _to_dataframe(records: list) -> pd.DataFrame:
    df = pd.DataFrame(records)
    if "ts_ns" in df.columns:
        df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df


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
        seen[stem] = f  # .zst comes after .mpack in sort order, wins

    return pd.concat(
        (read_mpack_zst(f) if f.suffix == ".zst" else read_mpack(f)
         for f in sorted(seen.values())),
        ignore_index=True,
    )


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "data"
    df = load(target)
    print(df[["ts", "exchange", "symbol", "best_bid_px", "best_ask_px", "spread", "wmid"]].head(20))
    print(f"\n{len(df)} records loaded")
