# Velociraptor MCP monitoring server

A small Python [Model Context Protocol](https://modelcontextprotocol.io) server that exposes the velociraptor systemd units + host system metrics as typed tools. Drop it next to the four recorder/fetcher services on the Linux box; talk to it from Claude Code (or any MCP client) over an SSH tunnel.

All tools return human-readable text reports. Each report-style tool has a companion `*_json` tool that returns the raw dict for programmatic use.

> Linux only for the systemctl tools. The psutil tools work on macOS too — useful for testing the server shape locally.

## Tools

| Tool | What it does |
|---|---|
| `list_units` | List the four velociraptor unit names. |
| `unit_status(unit)` | Active state, PID, uptime, restart count. |
| `health_check()` | One-shot table of all four units. |
| `unit_journal(unit, lines=80)` | Last N journal lines. |
| `recorder_error_log(service, lines=40)` | Tail today's `/data/syslog/<service>/<date>.error.log` (recorders only). |
| `system_overview()` | CPU, memory, swap, disk, top-3 by CPU and RSS. |
| `process_metrics(unit)` | Per-process CPU/RSS/threads/FDs for the unit's MainPID. |
| `top_processes(n=10, by='cpu')` | Global top N. |
| `disk_usage(path='/data')` | Single-path disk usage. |
| `data_dirs_overview()` | All `/data/*` paths the velociraptor configs write to, in one table. |
| `restart_unit(unit)` | `sudo systemctl restart <unit>`. **Requires sudoers drop-in.** Validated against unit allowlist. |

## Install (Linux server)

```bash
cd /home/ben/velociraptor/mcp_monitor
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt

# Smoke test (HTTP on 127.0.0.1:8765, default)
.venv/bin/python monitoring_server.py
# Ctrl-C to stop
```

## Sudoers drop-in (required for `restart_unit`)

Without this, `restart_unit` returns a clear error and everything else still works. Install with `visudo` (mode 0440):

```bash
sudo visudo -f /etc/sudoers.d/velociraptor-mcp
```

Paste:

```
ben ALL=(root) NOPASSWD: /bin/systemctl restart velociraptor-polymarket-recorder.service, \
                          /bin/systemctl restart velociraptor-orderbook-recorder.service, \
                          /bin/systemctl restart velociraptor-price-to-beat-fetcher.service, \
                          /bin/systemctl restart velociraptor-asset-id-fetcher.service
```

Note: the rule is an **explicit list**, not a `velociraptor-*.service` glob — so a hypothetical future malicious unit name can't ride in. Verify:

```bash
sudo -n -l -U ben | grep velociraptor
# should list exactly the four restart commands
```

A copy of this rule is shipped at `deploy/systemd/sudoers.d/velociraptor-mcp`.

## Run as a systemd unit (recommended)

The MCP server itself benefits from the same crash-recovery story as the things it monitors. Unit file is at `deploy/systemd/velociraptor-mcp-monitor.service`:

```bash
sudo cp deploy/systemd/velociraptor-mcp-monitor.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now velociraptor-mcp-monitor.service
journalctl -u velociraptor-mcp-monitor.service -f      # audit log for restart_unit
```

## Use from Claude Code (laptop)

1. Open an SSH tunnel:
   ```bash
   ssh -L 8765:127.0.0.1:8765 ben@server
   ```
2. Register the server:
   ```bash
   claude mcp add --transport http velociraptor-monitor http://127.0.0.1:8765
   ```
3. In a Claude Code session: *"Are all velociraptor services healthy and what's the box's memory pressure?"* — should fire `health_check` + `system_overview` automatically.

## Security model (v1)

- **HTTP server binds to 127.0.0.1 only.** Not reachable from the network.
- **The SSH tunnel is the auth boundary.** No bearer tokens in v1 — anyone who can SSH as `ben` can already restart these services from a shell.
- **`restart_unit` is allowlisted at three layers:** in-process unit list, scoped sudoers rule, and explicit `/bin/systemctl` path.
- **No other mutations.** No stop, no deploy, no config edits in v1.

## Local development (macOS)

For testing the server's tool shapes without a Linux box:

```bash
python3 -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt

# stdio mode (for MCP Inspector)
npx @modelcontextprotocol/inspector python3 monitoring_server.py --stdio
```

systemctl tools return a "systemctl not available on this host" sentinel on macOS; psutil tools work normally and report metrics for the laptop.
