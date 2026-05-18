# Velociraptor systemd units

systemd service units for the four long-running binaries. Linux only (this
repo's dev machine is macOS, which has no systemd).

The units run the **pre-built release binaries** from `target/release/`
directly — not `cargo run`. You must `cargo build --release` before
starting/restarting them; see Install and Upgrade below.

| Unit | Binary (run with `WorkingDirectory=/opt/velociraptor`) |
|------|---------|
| `velociraptor-polymarket-recorder.service` | `target/release/polymarket_recorder --config configs/polymarket.yaml` |
| `velociraptor-orderbook-recorder.service`  | `target/release/orderbook_recorder --config configs/server.yaml` |
| `velociraptor-price-to-beat-fetcher.service` | `target/release/price_to_beat_fetcher --config configs/fetcher.yaml` |
| `velociraptor-asset-id-fetcher.service` | `target/release/asset_id_fetcher --config configs/fetcher.yaml` |

Each unit has an `ExecStartPre=test -x <binary>` guard so a missing build
fails fast with a clear error instead of a cryptic `203/EXEC`.

The two recorders write rotating log files via `libs::logging` — driven by the
`logging:` section of their config (`configs/polymarket.yaml` /
`configs/server.yaml`): `{logging.dir}/{service}/{YYYY-MM-DD}.log` plus a
`.error.log` for WARN+, mirrored to stdout (so `journalctl` still works). The
two fetchers log to stdout only (captured by journald).

`velociraptor.target` groups all four so they can be started/stopped together.

## Unit settings — what each directive does

Every `.service` is `Type=simple` with the same supervision policy:

| Directive | Value | Effect |
|---|---|---|
| `After=network-online.target` / `Wants=network-online.target` | — | Don't start until the network is actually up (these binaries open WebSocket/HTTP connections immediately). |
| `ExecStartPre=test -x target/release/<bin>` | — | Fail fast with a clear error if the release binary hasn't been built yet. |
| `Restart=always` | — | Restart on *any* exit — crash, panic, or clean exit. Because the binary is pre-built, a restart is instant (no recompile). |
| `RestartSec=5` | 5 s | Wait 5 s between restarts so a tight crash loop doesn't spin the CPU. |
| `KillSignal=SIGINT` | — | Stop with `SIGINT` (Ctrl-C), not `SIGTERM`, so the recorder can flush its MessagePack buffer and close the file cleanly. |
| `TimeoutStopSec=30` | 30 s | Give it 30 s to shut down after `SIGINT` before systemd `SIGKILL`s it. |
| `WantedBy=multi-user.target` | — | This is what makes `systemctl enable` wire the unit into normal boot. |

## Assumptions — edit the units if these differ on your server

- **Service user/group:** `velociraptor`
- **Repo location (WorkingDirectory):** `/opt/velociraptor`
- **Binaries at:** `/opt/velociraptor/target/release/<name>` (default workspace
  target dir; the units hardcode this absolute path in `ExecStart`)

The units exec the pre-built binary directly, so start/restart is instant and a
broken build can never take a running service down — it just blocks the *next*
start (caught by `ExecStartPre`). The trade-off: **you must `cargo build
--release` yourself** before the first start and after every code change;
nothing rebuilds automatically.

## Install

```bash
# 1. Create the service user and place the repo (one-time)
sudo useradd --system --create-home --shell /usr/sbin/nologin velociraptor
sudo git clone <repo-url> /opt/velociraptor
sudo chown -R velociraptor:velociraptor /opt/velociraptor

# 2. Install rust for the service user (one-time)
sudo -u velociraptor sh -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'

# 3. Build the release binaries — REQUIRED before first start; the units
#    exec target/release/<bin> directly and will not start without it.
sudo -u velociraptor sh -c 'cd /opt/velociraptor && ~/.cargo/bin/cargo build --release'

# 4. Install the units
sudo cp deploy/systemd/*.service deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload

# 5. Enable + start everything
sudo systemctl enable --now velociraptor.target
sudo systemctl enable --now \
  velociraptor-polymarket-recorder.service \
  velociraptor-orderbook-recorder.service \
  velociraptor-price-to-beat-fetcher.service \
  velociraptor-asset-id-fetcher.service
```

## Operate

```bash
# Status / logs
systemctl status velociraptor-orderbook-recorder.service
journalctl -u velociraptor-orderbook-recorder.service -f

# Restart one / all
sudo systemctl restart velociraptor-asset-id-fetcher.service
sudo systemctl restart velociraptor-*.service

# Stop everything
sudo systemctl stop velociraptor-*.service
```

## Upgrade workflow (after pulling new code)

```bash
cd /opt/velociraptor
sudo -u velociraptor git pull
# Build first — the OLD binaries keep running untouched during the compile.
sudo -u velociraptor sh -c 'cd /opt/velociraptor && ~/.cargo/bin/cargo build --release'
# Only now swap to the new binaries.
sudo systemctl restart velociraptor-*.service
```

This ordering gives near-zero downtime: the running services keep using the
old `target/release/` binaries throughout the (possibly slow) build, and the
`restart` only happens once the new binaries are in place. If the build
*fails*, the old binaries are untouched and the services keep running on the
old code — you simply don't restart until the build is green. Restart is a
sub-second exec swap, not a recompile.

## Reboot & crash recovery

- **On OS reboot:** the four services auto-start, *provided you ran
  `systemctl enable` on them* (step 5). Each unit's
  `WantedBy=multi-user.target` is what wires it into boot — copying the files
  into `/etc/systemd/system/` alone does nothing until enabled.
- **`velociraptor.target` is a convenience grouping only.** It uses `Wants=`,
  and the services are pulled into boot by their own
  `WantedBy=multi-user.target` — not by the target. Enabling the four services
  is what guarantees reboot persistence; the target just lets you
  start/stop/restart all four with one command.
- **On process crash/exit (OS still up):** `Restart=always` brings the process
  back after `RestartSec=5`. This is independent of boot persistence — you need
  both, and the units have both.

### Crash-loop rate limit

systemd's default start-rate limit is 5 starts within 10 s. With pre-built
binaries a compile error can no longer cause this (the build is separate), but
it still triggers if the binary itself crashes immediately on every start —
e.g. a bad config, an unreachable Redis, a panic on startup, or a missing
`target/release/` build (the `ExecStartPre` guard fails fast each time):

```
systemctl status velociraptor-orderbook-recorder.service
#  Active: failed (Result: start-limit-hit)
```

Recover after fixing the root cause (rebuild, fix config, etc.):

```bash
sudo systemctl reset-failed velociraptor-orderbook-recorder.service
sudo systemctl start velociraptor-orderbook-recorder.service
```

To make boot recovery more forgiving, add to the `[Service]` section of a unit
(then `daemon-reload` + restart):

```ini
StartLimitIntervalSec=0   # never enter the start-limit-hit state
# or a sane bound instead:
# StartLimitIntervalSec=300
# StartLimitBurst=10
```

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `status=200/CHDIR` | `WorkingDirectory=/opt/velociraptor` doesn't exist or isn't readable by the service user. |
| `ExecStartPre` failed / `203/EXEC` | Release binary not built. Run `cargo build --release` (the `target/release/<bin>` is missing or not executable). |
| `217/USER` | The `velociraptor` user/group doesn't exist (`useradd` step skipped). |
| `failed (Result: start-limit-hit)` | Binary crash-looping on startup (bad config, unreachable Redis, panic) or unbuilt. Inspect `journalctl -u <unit>`, fix the cause, then `reset-failed`. |
| Starts then exits 0 immediately | A *fetcher* may be a run-once job, not a daemon. If so, `Restart=always` will keep relaunching it on `RestartSec=5` — switch that unit to `Restart=on-failure`, or run it as a `systemd.timer` instead of an always-on service. Confirm the intended runtime model before deploying. |
| Config not found | `--config configs/...` is relative to `WorkingDirectory`. Confirm the YAML exists at `/opt/velociraptor/configs/`. |

Inspect a failing unit:

```bash
systemctl status velociraptor-asset-id-fetcher.service
journalctl -u velociraptor-asset-id-fetcher.service -n 100 --no-pager
sudo systemd-analyze verify /etc/systemd/system/velociraptor-asset-id-fetcher.service
```
