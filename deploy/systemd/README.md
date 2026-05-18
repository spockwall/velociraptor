# Velociraptor systemd units

systemd service units for the four long-running binaries. Linux only (this
repo's dev machine is macOS, which has no systemd).

| Unit | Command |
|------|---------|
| `velociraptor-polymarket-recorder.service` | `cargo run --bin polymarket_recorder --release -- --config configs/polymarket.yaml` |
| `velociraptor-orderbook-recorder.service`  | `cargo run --bin orderbook_recorder --release -- --config configs/server.yaml` |
| `velociraptor-price-to-beat-fetcher.service` | `cargo run --bin price_to_beat_fetcher --release -- --config configs/example.yaml` |
| `velociraptor-asset-id-fetcher.service` | `cargo run --bin asset_id_fetcher --release -- --config configs/example.yaml` |

`velociraptor.target` groups all four so they can be started/stopped together.

## Unit settings — what each directive does

Every `.service` is `Type=simple` with the same supervision policy:

| Directive | Value | Effect |
|---|---|---|
| `After=network-online.target` / `Wants=network-online.target` | — | Don't start until the network is actually up (these binaries open WebSocket/HTTP connections immediately). |
| `Restart=always` | — | Restart on *any* exit — crash, panic, or clean exit. |
| `RestartSec=5` | 5 s | Wait 5 s between restarts so a tight crash loop doesn't spin the CPU. |
| `KillSignal=SIGINT` | — | Stop with `SIGINT` (Ctrl-C), not `SIGTERM`, so the recorder can flush its MessagePack buffer and close the file cleanly. |
| `TimeoutStopSec=30` | 30 s | Give it 30 s to shut down after `SIGINT` before systemd `SIGKILL`s it. |
| `WantedBy=multi-user.target` | — | This is what makes `systemctl enable` wire the unit into normal boot. |

## Assumptions — edit the units if these differ on your server

- **Service user/group:** `velociraptor`
- **Repo location (WorkingDirectory):** `/opt/velociraptor`
- **cargo path:** `/home/velociraptor/.cargo/bin/cargo` (rustup default for that user)

The units run `cargo run --release`, so the first start after a code change
recompiles before the process comes up — startup can take several minutes and
`systemctl start` will block until the build finishes. Build the workspace once
with `cargo build --release` before enabling the units to avoid a long first
start.

## Install

```bash
# 1. Create the service user and place the repo (one-time)
sudo useradd --system --create-home --shell /usr/sbin/nologin velociraptor
sudo git clone <repo-url> /opt/velociraptor
sudo chown -R velociraptor:velociraptor /opt/velociraptor

# 2. Install rust for the service user (one-time)
sudo -u velociraptor sh -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'

# 3. Pre-build (recommended, avoids a multi-minute first start)
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
# Pre-build as the service user so the restart doesn't block on compilation
sudo -u velociraptor sh -c 'cd /opt/velociraptor && ~/.cargo/bin/cargo build --release'
sudo systemctl restart velociraptor-*.service
```

Pre-building before the restart matters: the units invoke `cargo run --release`,
so if you skip the build step the *restart itself* runs the compile while the
service is down. A slow build = long downtime; a compile error = the service
fails to start.

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

### Crash-loop rate limit (important with `cargo run`)

systemd's default start-rate limit is 5 starts within 10 s. Because these units
run `cargo run --release`, a **compile error turns into a start failure** — five
fast failures and systemd gives up:

```
systemctl status velociraptor-orderbook-recorder.service
#  Active: failed (Result: start-limit-hit)
```

Recover manually with:

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
| `203/EXEC` or "No such file or directory" | `cargo` not at `/home/velociraptor/.cargo/bin/cargo`. Fix the `ExecStart` + `Environment=PATH` paths. |
| `217/USER` | The `velociraptor` user/group doesn't exist (`useradd` step skipped). |
| `failed (Result: start-limit-hit)` | Crash-looping (often a compile/config error). Inspect `journalctl -u <unit>`, fix, then `reset-failed`. |
| Starts then exits 0 immediately | A *fetcher* may be a run-once job, not a daemon. If so, `Restart=always` will keep relaunching it on `RestartSec=5` — switch that unit to `Restart=on-failure`, or run it as a `systemd.timer` instead of an always-on service. Confirm the intended runtime model before deploying. |
| Config not found | `--config configs/...` is relative to `WorkingDirectory`. Confirm the YAML exists at `/opt/velociraptor/configs/`. |

Inspect a failing unit:

```bash
systemctl status velociraptor-asset-id-fetcher.service
journalctl -u velociraptor-asset-id-fetcher.service -n 100 --no-pager
sudo systemd-analyze verify /etc/systemd/system/velociraptor-asset-id-fetcher.service
```
