"""
Read recorder MessagePack snapshot files into a pandas DataFrame.

File format: [u32 LE length][msgpack bytes] repeated.
File layout:  {base_path}/{exchange}/{symbol}/{YYYY-MM-DD}.mpack

Usage:
    pip install msgpack pandas
    python scripts/read_mpack.py data/binance/BTCUSDT/2026-04-03.mpack
    python scripts/read_mpack.py data/binance/BTCUSDT/  # all files in directory
"""

import struct
import sys
from pathlib import Path

import msgpack
import pandas as pd


def read_mpack(path: Path) -> pd.DataFrame:
    records = []
    with open(path, "rb") as f:
        while header := f.read(4):
            (n,) = struct.unpack("<I", header)
            records.append(msgpack.unpackb(f.read(n), raw=False))
    df = pd.DataFrame(records)
    if "ts_ns" in df.columns:
        df.insert(0, "ts", pd.to_datetime(df.pop("ts_ns"), unit="ns", utc=True))
    return df


def load(target: str) -> pd.DataFrame:
    p = Path(target)
    if p.is_file():
        return read_mpack(p)
    files = sorted(p.glob("*.mpack"))
    if not files:
        raise FileNotFoundError(f"No .mpack files in {p}")
    return pd.concat((read_mpack(f) for f in files), ignore_index=True)


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "data"
    df = load(target)
    print(df[["ts", "exchange", "symbol", "best_bid_px", "best_ask_px", "spread", "wmid"]].head(20))
    print(f"\n{len(df)} records loaded")
