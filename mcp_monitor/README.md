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

## Multi-user access (one MCP service, many laptops)

The MCP server runs **once** on `office-ml` as user `ben` and binds to `127.0.0.1:8765`. Other teammates reach the same instance by opening their own SSH tunnel from their own laptop — no extra server-side configuration, no separate MCP processes per user. SSH itself is the auth boundary; if a user can SSH the box, they can use the MCP, and if they can't, they can't.

### Server-side prerequisites (sysadmin, one-time per teammate)

1. **Give the teammate an SSH login on `office-ml`.** Either their own Linux account (preferred — restart_unit calls then show up under their username in `journalctl -u sshd` and bash history) or shared `ben` (simpler, no per-user audit trail).
2. **Install their public key** in `~/.ssh/authorized_keys` of that account (`ssh-copy-id`, or paste manually).
3. **They must be able to reach `127.0.0.1:8765` from inside the server** — automatic if they have any shell on the box, since the MCP listens on loopback for all local users.

No sudoers change is needed for users who only want **read-only** tools. The `restart_unit` tool runs `sudo systemctl restart …` as the SSH-authenticated user, so add each new teammate to the `/etc/sudoers.d/velociraptor-mcp` drop-in if they should be allowed to restart:

```
ben,alice,bob ALL=(root) NOPASSWD: /bin/systemctl restart velociraptor-polymarket-recorder.service, \
                                    /bin/systemctl restart velociraptor-orderbook-recorder.service, \
                                    /bin/systemctl restart velociraptor-price-to-beat-fetcher.service, \
                                    /bin/systemctl restart velociraptor-asset-id-fetcher.service
```

**Important:** the MCP service file currently has `User=ben` — so `restart_unit` calls always run as `ben` regardless of who is on the laptop end of the tunnel. Per-user sudo above only matters if you later switch the MCP server to run-as-invoker; until then, treat `restart_unit` as a shared button that anyone with tunnel access can press, and rely on `journalctl -u velociraptor-mcp-monitor.service` for the audit log.

### Client-side per-teammate setup (laptop)

Each teammate, on their own laptop:

```bash
# 1. Open the tunnel (replace key path + username with theirs)
ssh -fN -i ~/.ssh/<their-key> -L 8765:127.0.0.1:8765 <user>@office-ml.teahouse.finance

# 2. Register with Claude Code (one-time)
claude mcp add --transport http velociraptor-monitor http://127.0.0.1:8765

# Verify
curl -sS -o /dev/null -w "HTTP %{http_code}\n" http://127.0.0.1:8765/mcp
# expect HTTP 406
```

Optional: bake the tunnel into `~/.ssh/config` so `ssh office-ml` opens it automatically:

```
Host office-ml
    HostName office-ml.teahouse.finance
    User <their-user>
    IdentityFile ~/.ssh/<their-key>
    LocalForward 8765 127.0.0.1:8765
```

### Port-conflict gotcha

If two teammates ever pair on one laptop and someone else's tunnel is already on `8765`, the second `ssh -L 8765:…` fails with `bind: Address already in use`. Either kill the old tunnel (`pkill -f "8765:127.0.0.1"`) or use a different local port and tell Claude Code:

```bash
ssh -fN -L 9765:127.0.0.1:8765 <user>@office-ml.teahouse.finance
claude mcp add --transport http velociraptor-monitor http://127.0.0.1:9765
```

The server-side port stays `8765`; only the laptop-side port changes.

### When this stops being enough

The SSH-tunnel model works as long as everyone who needs MCP access is allowed to have an SSH shell on the server. If you ever need to give MCP access to someone who **shouldn't** have shell (e.g. a read-only stakeholder), switch the server to bind on the LAN (`0.0.0.0:8765`) and add bearer-token auth — a separate v2 effort.

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
