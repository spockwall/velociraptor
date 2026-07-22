"""
Read Kalshi CF Benchmarks recorder files into a pandas DataFrame.

The recorder writes one length-prefixed MessagePack stream per index:

    {base_path}/cfbenchmarks/{index_id}/{YYYY-MM-DD}.mpack
    ...and compressed variants ending in .mpack.zst

The nested source and average objects are flattened into columns. Exact
benchmark values remain available as strings (`source_value`,
`avg_60s_value`, and `last_60s_15min_value`); numeric convenience columns are
also added as `value`, `avg_60s`, and `last_60s_15min`.

Usage:
    pip install msgpack pandas zstandard

    # One day
    python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/BRTI/2026-07-22.mpack

    # All days for one index, or all indices below cfbenchmarks/
    python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/BRTI/
    python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/

    # Per-index/day summary
    python scripts/read_cf_benchmark.py data/kalshi/cfbenchmarks/ --summary
"""

import argparse
import io
import struct
import sys
from pathlib import Path

import msgpack
import pandas as pd


def _iter_records(buf):
    """Yield records from a length-prefixed MessagePack stream."""
    while header := buf.read(4):
        if len(header) < 4:
            print("warning: truncated length prefix at EOF -- skipped", file=sys.stderr)
            return

        (length,) = struct.unpack("<I", header)
        payload = buf.read(length)
        if len(payload) < length:
            print(
                f"warning: truncated final record (wanted {length} bytes, "
                f"got {len(payload)}) -- skipped",
                file=sys.stderr,
            )
            return
        yield msgpack.unpackb(payload, raw=False)


def _read_file(path: Path) -> list[dict]:
    if path.suffix == ".zst":
        try:
            import zstandard
        except ImportError as error:
            raise RuntimeError(
                "reading .zst files requires the 'zstandard' package"
            ) from error

        dctx = zstandard.ZstdDecompressor()
        with path.open("rb") as file_handle:
            with dctx.stream_reader(file_handle) as reader:
                return list(_iter_records(io.BufferedReader(reader)))

    with path.open("rb") as file_handle:
        return list(_iter_records(file_handle))


def _discover_files(root: Path) -> list[Path]:
    """Find recorder files recursively, preferring zstd when both exist."""
    candidates: dict[str, Path] = {}
    for path in sorted(root.rglob("*.mpack")) + sorted(root.rglob("*.mpack.zst")):
        stem = path.name.removesuffix(".zst").removesuffix(".mpack")
        candidates[str(path.parent / stem)] = path
    return sorted(candidates.values())


def _file_metadata(path: Path) -> tuple[str | None, str]:
    index_id = path.parent.name or None
    date = path.name.removesuffix(".zst").removesuffix(".mpack")
    return index_id, date


def _flatten_record(record: dict) -> dict:
    row = dict(record)
    source = row.pop("source_data", None) or {}
    average = row.pop("avg_60s_data", None) or {}
    quarter_hour = row.pop("last_60s_windowed_average_15min", None) or {}

    row.update(
        {
            "source_type": source.get("type"),
            "source_id": source.get("id"),
            "source_time_ms": source.get("time"),
            "source_value": source.get("value"),
            "avg_60s_value": average.get("value"),
            "avg_60s_window_size": average.get("window_size"),
            "avg_60s_window_start_ts_ms": average.get("window_start_ts_ms"),
            "avg_60s_window_end_ts_exclusive": average.get(
                "window_end_ts_exclusive"
            ),
            "last_60s_15min_value": quarter_hour.get("value"),
            "last_60s_15min_window_size": quarter_hour.get("window_size"),
            "last_60s_15min_window_start_ts_ms": quarter_hour.get(
                "window_start_ts_ms"
            ),
            "last_60s_15min_window_end_ts_exclusive": quarter_hour.get(
                "window_end_ts_exclusive"
            ),
        }
    )
    return row


def _to_dataframe(records: list[dict]) -> pd.DataFrame:
    if not records:
        return pd.DataFrame()

    df = pd.DataFrame(_flatten_record(record) for record in records)

    if "recv_timestamp" in df.columns:
        df.insert(
            0, "ts", pd.to_datetime(df["recv_timestamp"], unit="ns", utc=True)
        )
    if "source_time_ms" in df.columns:
        df["source_ts"] = pd.to_datetime(df["source_time_ms"], unit="ms", utc=True)
    if "received_at" in df.columns:
        df["kalshi_received_ts"] = pd.to_datetime(
            df["received_at"], unit="ms", utc=True
        )

    numeric_values = {
        "source_value": "value",
        "avg_60s_value": "avg_60s",
        "last_60s_15min_value": "last_60s_15min",
    }
    for source_column, numeric_column in numeric_values.items():
        if source_column in df.columns:
            df[numeric_column] = pd.to_numeric(df[source_column], errors="coerce")

    if {"received_at", "source_time_ms"}.issubset(df.columns):
        df["source_to_kalshi_ms"] = df["received_at"] - df["source_time_ms"]
    if {"recv_timestamp", "received_at"}.issubset(df.columns):
        df["kalshi_to_local_ms"] = (
            df["recv_timestamp"] - df["received_at"] * 1_000_000
        ) / 1e6
    if {"recv_timestamp", "source_time_ms"}.issubset(df.columns):
        df["end_to_end_ms"] = (
            df["recv_timestamp"] - df["source_time_ms"] * 1_000_000
        ) / 1e6

    return df


def load(target: str, index_id: str | None = None) -> pd.DataFrame:
    """Load one file or all recorder files recursively below a directory."""
    path = Path(target)
    if not path.exists():
        raise FileNotFoundError(f"Path does not exist: {path}")

    files = [path] if path.is_file() else _discover_files(path)
    if not files:
        raise FileNotFoundError(f"No .mpack or .mpack.zst files found under {path}")

    frames = []
    for file_path in files:
        records = _read_file(file_path)
        if not records:
            continue

        frame = _to_dataframe(records)
        path_index_id, date = _file_metadata(file_path)
        if "index_id" not in frame.columns:
            frame["index_id"] = path_index_id
        frame["date"] = date
        frame["source_file"] = str(file_path)
        frames.append(frame)

    if not frames:
        return pd.DataFrame()

    df = pd.concat(frames, ignore_index=True)
    if index_id is not None:
        df = df[df["index_id"] == index_id].reset_index(drop=True)
    if "recv_timestamp" in df.columns:
        df = df.sort_values("recv_timestamp", kind="stable").reset_index(drop=True)
    return df


def summary(target: str, index_id: str | None = None) -> pd.DataFrame:
    """Return record, value, and latency statistics for each index and day."""
    df = load(target, index_id=index_id)
    if df.empty:
        return df

    rows = []
    for (benchmark_id, date), group in df.groupby(["index_id", "date"]):
        row = {
            "index_id": benchmark_id,
            "date": date,
            "records": len(group),
            "first_ts": group["source_ts"].min(),
            "last_ts": group["source_ts"].max(),
            "first_value": group["value"].iloc[0],
            "last_value": group["value"].iloc[-1],
            "min_value": group["value"].min(),
            "max_value": group["value"].max(),
        }
        for column in (
            "source_to_kalshi_ms",
            "kalshi_to_local_ms",
            "end_to_end_ms",
        ):
            if column in group.columns:
                row[f"mean_{column}"] = group[column].mean()
        rows.append(row)

    return pd.DataFrame(rows).sort_values(["index_id", "date"]).reset_index(drop=True)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Read Kalshi CF Benchmarks recorder .mpack files",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "target",
        nargs="?",
        default="data/kalshi/cfbenchmarks",
        help="File or directory to load (default: data/kalshi/cfbenchmarks)",
    )
    parser.add_argument("--index", help="Only display one index ID, such as BRTI")
    parser.add_argument(
        "--summary", action="store_true", help="Print a per-index/day summary"
    )
    parser.add_argument(
        "--head", type=int, default=20, help="Rows to display (default: 20)"
    )
    args = parser.parse_args()

    df = summary(args.target, args.index) if args.summary else load(args.target, args.index)
    if df.empty:
        print("No records found.")
        return

    if args.summary:
        print(df.to_string(index=False))
        return

    display_columns = [
        "ts",
        "index_id",
        "value",
        "avg_60s",
        "last_60s_15min",
        "source_to_kalshi_ms",
        "kalshi_to_local_ms",
        "end_to_end_ms",
        "seq",
    ]
    display_columns = [column for column in display_columns if column in df.columns]
    print(df[display_columns].head(args.head).to_string(index=False))

    counts = df["index_id"].value_counts().to_dict()
    print(f"\n{len(df):,} records  |  {counts}")


if __name__ == "__main__":
    main()
