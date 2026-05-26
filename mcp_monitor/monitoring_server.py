"""Velociraptor MCP monitoring server.

Read-only health + system-metrics for the four velociraptor systemd units,
plus a single gated `restart_unit` mutation. All tools return human-readable
text reports; companion `*_json` tools expose the raw dicts.

Run:
    python monitoring_server.py            # Streamable HTTP on 127.0.0.1:8765
    python monitoring_server.py --stdio    # stdio (for MCP Inspector / dev)
"""

from __future__ import annotations

import argparse
import os
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import psutil
from mcp.server.fastmcp import FastMCP

# ---------------------------------------------------------------------------
# Configuration — single source of truth, mirrors deploy/systemd/ unit names.
# ---------------------------------------------------------------------------

UNITS: tuple[str, ...] = (
    "velociraptor-polymarket-recorder.service",
    "velociraptor-orderbook-recorder.service",
    "velociraptor-price-to-beat-fetcher.service",
    "velociraptor-asset-id-fetcher.service",
)

# Subset that uses libs::logging (file-backed error log under /data/syslog/).
# Fetchers log to journald only — they're not in this map.
RECORDER_LOG_SERVICES: dict[str, str] = {
    "velociraptor-polymarket-recorder.service": "polymarket_recorder",
    "velociraptor-orderbook-recorder.service": "orderbook_recorder",
}

DATA_PATHS: tuple[str, ...] = (
    "/data/syslog",
    "/data/orderbook_3",
    "/data/polymarket_3",
    "/data/asset_ids",
    "/data/price_to_beat",
)

SYSLOG_ROOT = Path("/data/syslog")

HTTP_HOST = "127.0.0.1"
HTTP_PORT = 8765

mcp = FastMCP("velociraptor-monitor")


# ---------------------------------------------------------------------------
# Formatting helpers
# ---------------------------------------------------------------------------

def _fmt_bytes(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} PB"


def _fmt_duration(seconds: float) -> str:
    seconds = int(seconds)
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{seconds // 60}m {seconds % 60}s"
    if seconds < 86400:
        h, rem = divmod(seconds, 3600)
        return f"{h}h {rem // 60}m"
    d, rem = divmod(seconds, 86400)
    h = rem // 3600
    return f"{d}d {h}h"


def _fmt_table(headers: list[str], rows: list[list[str]], indent: str = "  ") -> str:
    if not rows:
        return f"{indent}(no rows)"
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))
    lines = [indent + "  ".join(h.ljust(widths[i]) for i, h in enumerate(headers))]
    for row in rows:
        lines.append(indent + "  ".join(cell.ljust(widths[i]) for i, cell in enumerate(row)))
    return "\n".join(lines)


def _now_utc() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


# ---------------------------------------------------------------------------
# systemctl wrappers
# ---------------------------------------------------------------------------

@dataclass
class SystemctlMissing(Exception):
    msg: str = "systemctl not available on this host (not a systemd Linux box)"

    def __str__(self) -> str:
        return self.msg


def _systemctl_available() -> bool:
    return shutil.which("systemctl") is not None


def _require_systemctl() -> None:
    if not _systemctl_available():
        raise SystemctlMissing()


def _validate_unit(unit: str) -> None:
    if unit not in UNITS:
        raise ValueError(
            f"unit '{unit}' is not in the velociraptor allowlist. "
            f"Allowed: {', '.join(UNITS)}"
        )


def _systemctl_show(unit: str, *props: str) -> dict[str, str]:
    """Return parsed `systemctl show -p ... <unit>` as a dict."""
    cmd = ["systemctl", "show", unit, "--no-pager"]
    for p in props:
        cmd += ["-p", p]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
    out: dict[str, str] = {}
    for line in r.stdout.splitlines():
        if "=" in line:
            k, _, v = line.partition("=")
            out[k] = v
    return out


def _is_active(unit: str) -> str:
    r = subprocess.run(
        ["systemctl", "is-active", unit],
        capture_output=True, text=True, timeout=10,
    )
    # is-active returns non-zero for inactive/failed; we still want the word.
    return (r.stdout or r.stderr).strip() or "unknown"


# ---------------------------------------------------------------------------
# Internal dict-producing helpers (shared by report + _json tools)
# ---------------------------------------------------------------------------

def _unit_status_dict(unit: str) -> dict[str, Any]:
    _validate_unit(unit)
    _require_systemctl()
    props = _systemctl_show(
        unit,
        "ActiveState", "SubState", "MainPID", "NRestarts",
        "ActiveEnterTimestamp", "ActiveExitTimestamp",
        "Result", "ExecMainStatus",
    )
    main_pid = int(props.get("MainPID") or 0) or None
    since_str = props.get("ActiveEnterTimestamp", "")
    since_secs: float | None = None
    try:
        # systemd format: "Mon 2026-05-26 14:03:01 UTC"
        dt = datetime.strptime(since_str, "%a %Y-%m-%d %H:%M:%S %Z")
        since_secs = (datetime.utcnow() - dt).total_seconds()
    except (ValueError, TypeError):
        pass
    return {
        "unit": unit,
        "active": props.get("ActiveState", "unknown"),
        "sub": props.get("SubState", "unknown"),
        "main_pid": main_pid,
        "restarts": int(props.get("NRestarts") or 0),
        "since_str": since_str,
        "since_secs": since_secs,
        "result": props.get("Result", ""),
        "exec_status": props.get("ExecMainStatus", ""),
    }


def _system_overview_dict() -> dict[str, Any]:
    vm = psutil.virtual_memory()
    sm = psutil.swap_memory()
    try:
        load1, load5, load15 = os.getloadavg()
    except (OSError, AttributeError):
        load1 = load5 = load15 = 0.0
    disks = []
    seen = set()
    for part in psutil.disk_partitions(all=False):
        if part.mountpoint in seen:
            continue
        seen.add(part.mountpoint)
        try:
            u = psutil.disk_usage(part.mountpoint)
        except (PermissionError, OSError):
            continue
        disks.append({
            "mount": part.mountpoint,
            "total": u.total,
            "used": u.used,
            "free": u.free,
            "pct": u.percent,
        })
    return {
        "host": socket.gethostname(),
        "cpu_pct": psutil.cpu_percent(interval=0.3),
        "cpu_count": psutil.cpu_count(logical=True) or 1,
        "load_avg": (load1, load5, load15),
        "mem": {
            "total": vm.total, "used": vm.used,
            "available": vm.available, "pct": vm.percent,
        },
        "swap": {"total": sm.total, "used": sm.used, "pct": sm.percent},
        "disk": disks,
        "uptime_secs": time.time() - psutil.boot_time(),
        "boot_time": psutil.boot_time(),
    }


def _top_processes_dict(n: int, by: str) -> list[dict[str, Any]]:
    """Return top-N processes by 'cpu' or 'rss'."""
    procs = []
    # First pass: prime cpu_percent timers
    for p in psutil.process_iter(["pid", "name"]):
        try:
            p.cpu_percent(None)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    time.sleep(0.3)
    for p in psutil.process_iter(["pid", "name", "memory_info"]):
        try:
            info = p.info
            procs.append({
                "pid": info["pid"],
                "name": info["name"] or "?",
                "cpu_pct": p.cpu_percent(None),
                "rss": info["memory_info"].rss if info["memory_info"] else 0,
            })
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    key = "cpu_pct" if by == "cpu" else "rss"
    procs.sort(key=lambda x: x[key], reverse=True)
    return procs[:n]


def _process_metrics_dict(unit: str) -> dict[str, Any]:
    st = _unit_status_dict(unit)
    pid = st["main_pid"]
    if not pid:
        return {"unit": unit, "error": "no MainPID — unit is not running"}
    try:
        p = psutil.Process(pid)
        with p.oneshot():
            cpu = p.cpu_percent(interval=0.3)
            mem = p.memory_info()
            try:
                nfds = p.num_fds()
            except (AttributeError, psutil.AccessDenied):
                nfds = None
            return {
                "unit": unit, "pid": pid, "status": p.status(),
                "cpu_pct": cpu,
                "rss": mem.rss, "vms": mem.vms,
                "num_threads": p.num_threads(),
                "num_fds": nfds,
                "create_time": p.create_time(),
                "age_secs": time.time() - p.create_time(),
            }
    except psutil.NoSuchProcess:
        return {"unit": unit, "error": f"PID {pid} vanished between status and probe"}


# ---------------------------------------------------------------------------
# Formatters: dict -> human-readable str
# ---------------------------------------------------------------------------

_STATE_TAG = {
    "active": "[OK]",
    "activating": "[WARN]",
    "deactivating": "[WARN]",
    "inactive": "[STOP]",
    "failed": "[FAIL]",
}


def _fmt_unit_status(d: dict[str, Any]) -> str:
    tag = _STATE_TAG.get(d["active"], "[?]")
    since = _fmt_duration(d["since_secs"]) if d["since_secs"] else "unknown"
    lines = [
        f"Unit: {d['unit']}  {tag}",
        f"  State:     {d['active']} ({d['sub']})",
        f"  PID:       {d['main_pid'] or '-'}",
        f"  Up for:    {since}",
        f"  Restarts:  {d['restarts']}",
    ]
    if d["result"] and d["result"] != "success":
        lines.append(f"  Result:    {d['result']} (exec status {d['exec_status']})")
    return "\n".join(lines)


def _fmt_health_check(rows: list[dict[str, Any]]) -> str:
    headers = ["UNIT", "STATE", "SINCE", "RESTARTS"]
    table_rows = []
    healthy = 0
    for r in rows:
        tag = _STATE_TAG.get(r["active"], "[?]")
        if r["active"] == "active":
            healthy += 1
        since = _fmt_duration(r["since_secs"]) if r["since_secs"] else "-"
        table_rows.append([r["unit"], tag, since, str(r["restarts"])])
    out = [f"Velociraptor service health  ({_now_utc()})", ""]
    out.append(_fmt_table(headers, table_rows))
    out.append("")
    out.append(f"Summary: {healthy}/{len(rows)} healthy.")
    failed = [r["unit"] for r in rows if r["active"] == "failed"]
    if failed:
        out.append(f"Failed: {', '.join(failed)} — see `unit_journal` for details.")
    return "\n".join(out)


def _fmt_system_overview(d: dict[str, Any], top_cpu: list[dict[str, Any]],
                        top_rss: list[dict[str, Any]]) -> str:
    mem = d["mem"]
    sw = d["swap"]
    la = d["load_avg"]
    lines = [
        f"System overview  (host: {d['host']}, up {_fmt_duration(d['uptime_secs'])})",
        "",
        f"  CPU      {d['cpu_pct']:5.1f}%   "
        f"(load {la[0]:.2f}  {la[1]:.2f}  {la[2]:.2f} / {d['cpu_count']} cores)",
        f"  Memory   {mem['pct']:5.1f}%   "
        f"used {_fmt_bytes(mem['used'])} / {_fmt_bytes(mem['total'])}     "
        f"available {_fmt_bytes(mem['available'])}",
    ]
    if sw["total"]:
        lines.append(
            f"  Swap     {sw['pct']:5.1f}%   "
            f"used {_fmt_bytes(sw['used'])} / {_fmt_bytes(sw['total'])}"
        )
    else:
        lines.append("  Swap      none")
    lines.append("  Disk")
    for disk in d["disk"]:
        lines.append(
            f"           {disk['mount']:<14} {disk['pct']:5.1f}%  "
            f"free {_fmt_bytes(disk['free'])} / {_fmt_bytes(disk['total'])}"
        )
    if top_cpu:
        cpu_str = " / ".join(f"{p['name']} {p['cpu_pct']:.1f}%" for p in top_cpu[:3])
        lines.append("")
        lines.append(f"Top by CPU: {cpu_str}")
    if top_rss:
        rss_str = " / ".join(f"{p['name']} {_fmt_bytes(p['rss'])}" for p in top_rss[:3])
        lines.append(f"Top by RSS: {rss_str}")
    return "\n".join(lines)


def _fmt_process_metrics(d: dict[str, Any]) -> str:
    if "error" in d:
        return f"Process metrics for {d['unit']}: {d['error']}"
    return "\n".join([
        f"Process metrics: {d['unit']}",
        f"  PID:        {d['pid']} ({d['status']})",
        f"  CPU:        {d['cpu_pct']:.1f}%",
        f"  RSS:        {_fmt_bytes(d['rss'])}",
        f"  VMS:        {_fmt_bytes(d['vms'])}",
        f"  Threads:    {d['num_threads']}",
        f"  FDs:        {d['num_fds'] if d['num_fds'] is not None else 'n/a'}",
        f"  Age:        {_fmt_duration(d['age_secs'])}",
    ])


def _fmt_top_processes(rows: list[dict[str, Any]], by: str) -> str:
    by_label = "CPU%" if by == "cpu" else "RSS"
    headers = ["PID", "NAME", "CPU%", "RSS"]
    table_rows = [
        [str(p["pid"]), p["name"][:30], f"{p['cpu_pct']:.1f}", _fmt_bytes(p["rss"])]
        for p in rows
    ]
    return "\n".join([
        f"Top {len(rows)} processes by {by_label}  ({_now_utc()})",
        "",
        _fmt_table(headers, table_rows),
    ])


# ---------------------------------------------------------------------------
# Tools — service health
# ---------------------------------------------------------------------------

@mcp.tool()
def list_units() -> str:
    """List the velociraptor systemd units this server manages."""
    lines = [f"Velociraptor units ({len(UNITS)} total):", ""]
    for u in UNITS:
        lines.append(f"  - {u}")
    return "\n".join(lines)


@mcp.tool()
def unit_status(unit: str) -> str:
    """Detailed status of one velociraptor unit. Pass the full unit name."""
    try:
        return _fmt_unit_status(_unit_status_dict(unit))
    except (SystemctlMissing, ValueError) as e:
        return f"Error: {e}"


@mcp.tool()
def unit_status_json(unit: str) -> dict[str, Any]:
    """Same as `unit_status` but returns the raw dict."""
    try:
        return _unit_status_dict(unit)
    except (SystemctlMissing, ValueError) as e:
        return {"error": str(e)}


@mcp.tool()
def health_check() -> str:
    """One-shot health report for all four velociraptor units."""
    if not _systemctl_available():
        return f"Error: {SystemctlMissing()}"
    rows = []
    for u in UNITS:
        try:
            rows.append(_unit_status_dict(u))
        except Exception as e:  # noqa: BLE001
            rows.append({"unit": u, "active": "unknown", "sub": "",
                         "main_pid": None, "restarts": 0,
                         "since_secs": None, "result": str(e),
                         "exec_status": ""})
    return _fmt_health_check(rows)


@mcp.tool()
def health_check_json() -> list[dict[str, Any]]:
    """Raw dict version of `health_check`."""
    if not _systemctl_available():
        return [{"error": str(SystemctlMissing())}]
    return [_unit_status_dict(u) for u in UNITS]


@mcp.tool()
def unit_journal(unit: str, lines: int = 80) -> str:
    """Return the last N journal lines for a velociraptor unit."""
    try:
        _validate_unit(unit)
        _require_systemctl()
    except (SystemctlMissing, ValueError) as e:
        return f"Error: {e}"
    lines = max(1, min(lines, 1000))
    r = subprocess.run(
        ["journalctl", "-u", unit, "-n", str(lines), "--no-pager"],
        capture_output=True, text=True, timeout=20,
    )
    body = (r.stdout or r.stderr).strip() or "(no journal output)"
    return f"Last {lines} journal lines: {unit}\n\n{body}"


@mcp.tool()
def recorder_error_log(service: str, lines: int = 40) -> str:
    """Tail today's error log for a recorder service.

    `service` is the libs::logging service name, e.g. 'polymarket_recorder'
    or 'orderbook_recorder'. Fetchers don't have error logs (journald only).
    """
    if service not in RECORDER_LOG_SERVICES.values():
        return (f"Error: '{service}' has no on-disk error log. "
                f"Valid: {', '.join(RECORDER_LOG_SERVICES.values())}")
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    path = SYSLOG_ROOT / service / f"{today}.error.log"
    if not path.exists():
        return f"No error log at {path} (file does not exist — that's a good sign)."
    lines = max(1, min(lines, 500))
    try:
        with path.open("r", errors="replace") as f:
            tail = f.readlines()[-lines:]
    except OSError as e:
        return f"Error reading {path}: {e}"
    return f"Last {len(tail)} lines of {path}:\n\n{''.join(tail)}"


# ---------------------------------------------------------------------------
# Tools — system metrics
# ---------------------------------------------------------------------------

@mcp.tool()
def system_overview() -> str:
    """CPU, memory, swap, disk, and top-3 processes by CPU and RSS."""
    d = _system_overview_dict()
    top_cpu = _top_processes_dict(3, "cpu")
    top_rss = _top_processes_dict(3, "rss")
    return _fmt_system_overview(d, top_cpu, top_rss)


@mcp.tool()
def system_overview_json() -> dict[str, Any]:
    """Raw dict version of `system_overview`."""
    return {
        "overview": _system_overview_dict(),
        "top_cpu": _top_processes_dict(3, "cpu"),
        "top_rss": _top_processes_dict(3, "rss"),
    }


@mcp.tool()
def process_metrics(unit: str) -> str:
    """Per-process CPU/RSS/threads/FDs for the MainPID of a velociraptor unit."""
    try:
        return _fmt_process_metrics(_process_metrics_dict(unit))
    except (SystemctlMissing, ValueError) as e:
        return f"Error: {e}"


@mcp.tool()
def process_metrics_json(unit: str) -> dict[str, Any]:
    """Raw dict version of `process_metrics`."""
    try:
        return _process_metrics_dict(unit)
    except (SystemctlMissing, ValueError) as e:
        return {"error": str(e)}


@mcp.tool()
def top_processes(n: int = 10, by: str = "cpu") -> str:
    """Top N processes by 'cpu' or 'rss'."""
    n = max(1, min(n, 50))
    by = "cpu" if by not in ("cpu", "rss") else by
    return _fmt_top_processes(_top_processes_dict(n, by), by)


@mcp.tool()
def disk_usage(path: str = "/data") -> str:
    """Disk usage for a path (default /data). Velociraptor cares about /data/*."""
    if not Path(path).exists():
        return f"Error: {path} does not exist."
    try:
        u = psutil.disk_usage(path)
    except OSError as e:
        return f"Error reading disk usage for {path}: {e}"
    return "\n".join([
        f"Disk usage: {path}",
        f"  Total:   {_fmt_bytes(u.total)}",
        f"  Used:    {_fmt_bytes(u.used)}  ({u.percent:.1f}%)",
        f"  Free:    {_fmt_bytes(u.free)}",
    ])


@mcp.tool()
def data_dirs_overview() -> str:
    """Report on all known /data/* paths the velociraptor configs write to."""
    headers = ["PATH", "EXISTS", "USED%", "FREE"]
    rows = []
    for p in DATA_PATHS:
        path = Path(p)
        if not path.exists():
            rows.append([p, "no", "-", "-"])
            continue
        try:
            u = psutil.disk_usage(p)
            rows.append([p, "yes", f"{u.percent:.1f}", _fmt_bytes(u.free)])
        except OSError as e:
            rows.append([p, "err", str(e)[:20], "-"])
    return f"Velociraptor /data paths  ({_now_utc()})\n\n" + _fmt_table(headers, rows)


# ---------------------------------------------------------------------------
# Tools — mutation (gated)
# ---------------------------------------------------------------------------

@mcp.tool()
def restart_unit(unit: str) -> str:
    """Restart a velociraptor unit via `sudo systemctl restart`.

    Requires NOPASSWD sudoers rule (see README). Validated against the fixed
    unit allowlist before sudo is invoked.
    """
    try:
        _validate_unit(unit)
        _require_systemctl()
    except (SystemctlMissing, ValueError) as e:
        return f"Error: {e}"

    # Audit log to stderr — journald captures this when run as a systemd unit.
    print(f"[{_now_utc()}] restart_unit: {unit}", file=sys.stderr, flush=True)

    r = subprocess.run(
        ["sudo", "-n", "/bin/systemctl", "restart", unit],
        capture_output=True, text=True, timeout=30,
    )
    if r.returncode != 0:
        return (f"restart_unit failed (rc={r.returncode}):\n"
                f"stdout: {r.stdout.strip()}\n"
                f"stderr: {r.stderr.strip()}\n"
                f"Hint: check sudoers drop-in at /etc/sudoers.d/velociraptor-mcp.")

    # Give systemd a moment to settle, then report fresh status.
    time.sleep(1.0)
    try:
        return f"Restarted {unit}.\n\n" + _fmt_unit_status(_unit_status_dict(unit))
    except Exception as e:  # noqa: BLE001
        return f"Restarted {unit} (rc=0) but failed to fetch fresh status: {e}"


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Velociraptor MCP monitor.")
    parser.add_argument("--stdio", action="store_true",
                        help="Use stdio transport (default: Streamable HTTP).")
    parser.add_argument("--host", default=HTTP_HOST,
                        help=f"HTTP bind host (default {HTTP_HOST}).")
    parser.add_argument("--port", type=int, default=HTTP_PORT,
                        help=f"HTTP bind port (default {HTTP_PORT}).")
    args = parser.parse_args()

    if args.stdio:
        mcp.run("stdio")
    else:
        # FastMCP reads host/port from settings; set them via env or kwargs.
        mcp.settings.host = args.host
        mcp.settings.port = args.port
        mcp.run("streamable-http")


if __name__ == "__main__":
    main()
