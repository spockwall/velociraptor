"""Engine-side append-only CSV log for actions + events.

Two daily-rotated CSV files under `base_dir`:

    {base_dir}/actions/{YYYY-MM-DD}.csv   — orders the engine *initiated*
    {base_dir}/events/{YYYY-MM-DD}.csv    — events the engine *received*

CSV is intentionally human-readable: `tail -f`, open in Excel, grep, etc.
Records carry a wall-clock timestamp + the relevant payload fields as
named columns. Both files are append-only with one header row per file.

Schema (loose; consumers should treat extra fields as optional):

    actions/*.csv columns:
        ts, ts_ns, kind, exchange, strategy, label, side, symbol,
        px, qty, client_oid, exchange_oid, ok, latency_ms, error, count

    events/*.csv columns:
        ts, ts_ns, topic, type, exchange, symbol, side, px, qty,
        filled, status, fee, taker_oid, client_oid, exchange_oid,
        maker_orders

Variant-irrelevant fields are blank cells (e.g. `fee` on order_update,
`filled`/`status` on fill). `maker_orders` is a Polymarket-only
exchange-specific blob.
"""

from __future__ import annotations

import csv
import datetime as dt
import json
import logging
import threading
import time
from pathlib import Path
from typing import Any, Optional

log = logging.getLogger(__name__)


# Header columns; order is the order they'll appear in the file.
_ACTION_COLS = [
    "ts",
    "ts_ns",
    "kind",
    "exchange",
    "strategy",
    "label",
    "side",
    "symbol",
    "px",
    "qty",
    "client_oid",
    "exchange_oid",
    "ok",
    "latency_ms",
    "error",
    "count",
    # `place_market` only — passed via `extra={"tif": ...}`.
    "tif",
]

_EVENT_COLS = [
    "ts",
    "ts_ns",
    "topic",
    "type",
    "exchange",
    "symbol",
    "side",
    "px",
    "qty",
    # order_update-only
    "filled",
    "status",
    # fill-only
    "fee",
    "taker_oid",
    # both (when known)
    "client_oid",
    "exchange_oid",
    # fill-only (Polymarket emits a UUID per matched trade; the venue
    # does NOT publish an order id on fills, so use this + `taker_oid` +
    # `maker_orders` to match fills back to placed orders).
    "trade_id",
    # fill-only, exchange-specific blob (Polymarket maker_orders list,
    # JSON-encoded by the recorder; for events the value is whatever the
    # `event` payload carried, msgpack-decoded into a Python object).
    "maker_orders",
]


class _OpenFile:
    """Wraps the file handle + csv.DictWriter for one (kind, date) slot."""

    def __init__(self, path: Path, columns: list[str]):
        # Open in append mode; preserve existing rows on restart.
        preexisting = path.exists() and path.stat().st_size > 0
        # newline='' is the csv module's recommendation.
        self._fh = open(path, "a", encoding="utf-8", newline="")
        self.path = path
        self.writer = csv.DictWriter(
            self._fh, fieldnames=columns, extrasaction="ignore"
        )
        if not preexisting:
            self.writer.writeheader()

    def write(self, row: dict) -> None:
        self.writer.writerow(row)

    def flush(self) -> None:
        self._fh.flush()

    def close(self) -> None:
        try:
            self._fh.flush()
        finally:
            self._fh.close()


class EventLog:
    """Thread-safe daily-rotation CSV appender.

    `record_action` and `record_event` are safe to call from any thread;
    file handles are guarded by a mutex. A background thread flushes the
    Python buffer once per second.

    Pass `enabled=False` (or `base_dir=None`) to make every method a no-op.
    """

    def __init__(
        self,
        base_dir: Optional[str | Path],
        flush_interval_secs: float = 1.0,
        enabled: bool = True,
    ):
        self._enabled = enabled and base_dir is not None
        self._base = Path(base_dir) if base_dir else None
        self._flush_interval = flush_interval_secs
        # Keyed by (kind, date) where kind ∈ {"actions", "events"}.
        self._handles: dict[tuple[str, str], _OpenFile] = {}
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._flusher: Optional[threading.Thread] = None

        if self._enabled:
            self._base.mkdir(parents=True, exist_ok=True)  # type: ignore[union-attr]
            (self._base / "actions").mkdir(exist_ok=True)  # type: ignore[union-attr]
            (self._base / "events").mkdir(exist_ok=True)  # type: ignore[union-attr]
            self._flusher = threading.Thread(
                target=self._flush_loop, name="event-log-flusher", daemon=True
            )
            self._flusher.start()
            log.info(f"EventLog: writing CSV to {self._base}")

    # ── public API ──

    def record_action(
        self,
        kind: str,
        exchange: str,
        *,
        strategy: Optional[str] = None,
        label: Optional[str] = None,
        side: Optional[str] = None,
        symbol: Optional[str] = None,
        px: Optional[float] = None,
        qty: Optional[float] = None,
        client_oid: Optional[str] = None,
        exchange_oid: Optional[str] = None,
        ok: bool = True,
        latency_ms: Optional[float] = None,
        error: Optional[str] = None,
        extra: Optional[dict] = None,
    ) -> None:
        """Append one row to today's actions/ CSV.

        `kind` is one of `place`, `cancel`, `cancel_all`, `heartbeat`.
        Optional `extra` may carry well-known fields (e.g. `count` for
        cancel results) — anything not in `_ACTION_COLS` is dropped.
        """
        if not self._enabled:
            return
        row: dict[str, Any] = {
            **_now_fields(),
            "kind": kind,
            "exchange": exchange,
            "strategy": strategy,
            "label": label,
            "side": side,
            "symbol": symbol,
            "px": px,
            "qty": qty,
            "client_oid": client_oid,
            "exchange_oid": exchange_oid,
            "ok": ok,
            "latency_ms": latency_ms,
            "error": error,
        }
        if extra:
            for k in _ACTION_COLS:
                if k in extra and k not in row:
                    row[k] = extra[k]
            # Also accept count specifically — common extra for cancel_all.
            if "count" in extra:
                row["count"] = extra["count"]
        self._append("actions", row)

    def record_event(self, topic: str, event: dict) -> None:
        """Append one user-channel event to today's events/ CSV. The inner
        `event` dict (UserEvent variant) is flattened — only its leaf
        fields appear as columns. Topic is preserved separately."""
        if not self._enabled:
            return
        row: dict[str, Any] = {
            **_now_fields(),
            "topic": topic,
        }
        # The UserEvent is internally tagged with "type" + flat scalar fields,
        # so the dict-merge below already flattens correctly.
        if isinstance(event, dict):
            for k, v in event.items():
                if k not in _EVENT_COLS:
                    continue
                # `maker_orders` arrives as a list[dict] from msgpack.
                # JSON-encode it so the CSV cell is a clean parseable
                # blob (Python repr would use single quotes and break
                # `json.loads` round-trips).
                if k == "maker_orders" and isinstance(v, list):
                    row[k] = json.dumps(v, default=str)
                else:
                    row[k] = v
            # Some Polymarket fills carry ts_ns inside the event; prefer
            # ours (wall clock at receive) for ordering.
            # The event's own ts_ns is lost — that's fine, the on-disk
            # `ts_ns` column captures when we *saw* the event.
        self._append("events", row)

    def close(self) -> None:
        if not self._enabled:
            return
        self._stop.set()
        if self._flusher is not None:
            self._flusher.join(timeout=2)
        with self._lock:
            for handle in self._handles.values():
                try:
                    handle.close()
                except Exception:  # noqa: BLE001
                    log.exception("EventLog: close handle failed")
            self._handles.clear()

    # ── inner ──

    def _append(self, kind: str, row: dict) -> None:
        today = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d")
        with self._lock:
            handle = self._open(kind, today)
            try:
                handle.write(row)
            except Exception:  # noqa: BLE001
                log.exception(f"EventLog: write failed for {kind}")

    def _open(self, kind: str, date: str) -> _OpenFile:
        # Drop stale handle for this `kind` if the date rolled over.
        stale_keys = [(k, d) for (k, d) in self._handles if k == kind and d != date]
        for stale in stale_keys:
            handle = self._handles.pop(stale)
            try:
                handle.close()
            except Exception:  # noqa: BLE001
                log.exception("EventLog: rotation close failed")

        key = (kind, date)
        handle = self._handles.get(key)
        if handle is not None:
            return handle

        path = self._base / kind / f"{date}.csv"  # type: ignore[union-attr]
        columns = _ACTION_COLS if kind == "actions" else _EVENT_COLS
        handle = _OpenFile(path, columns)
        self._handles[key] = handle
        log.info(f"EventLog: opened {path}")
        return handle

    def _flush_loop(self) -> None:
        while not self._stop.wait(self._flush_interval):
            with self._lock:
                for handle in self._handles.values():
                    try:
                        handle.flush()
                    except Exception:  # noqa: BLE001
                        log.exception("EventLog: periodic flush failed")


def _now_fields() -> dict[str, Any]:
    """Wall-clock fields appearing in every record."""
    now = time.time_ns()
    return {
        "ts": dt.datetime.fromtimestamp(now / 1e9, tz=dt.timezone.utc).isoformat(),
        "ts_ns": now,
    }
