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

## Install (first-time setup)

The units hardcode three things — service user `velociraptor`, repo at
`/opt/velociraptor`, binaries at `/opt/velociraptor/target/release/`. **All
three must exist before the units can start.** Skipping the user step is the
most common failure: `ExecStartPre` aborts with `status=217/USER` (systemd
can't resolve `User=velociraptor`) and *every* unit fails identically.

```bash
# 1. Create the service user (REQUIRED — missing user => 217/USER on all units)
sudo useradd --system --create-home --shell /usr/sbin/nologin velociraptor

# 2. Place the repo at /opt/velociraptor and hand it to the service user.
#    Fresh clone:
sudo git clone <repo-url> /opt/velociraptor
#    …or copy an existing working clone (preserves your edited configs):
#    sudo cp -a /home/<you>/velociraptor /opt/velociraptor
sudo chown -R velociraptor:velociraptor /opt/velociraptor

# 3. Pre-create EVERY /data path the services write to and hand them to the
#    service user. /data is root-owned; the unprivileged `velociraptor` user
#    cannot write there until you chown. Two distinct kinds of path:
#      - logs:    {logging.dir}        — both recorders PANIC on startup if
#                                        they can't create it
#      - storage: {storage.base_path}  — orderbook_recorder errors per-file
#                                        ("StorageWriter: ... Permission denied")
#    Paths come from the configs (adjust if you changed them):
#      configs/server.yaml     -> logging.dir /data/syslog, storage /data/orderbook_2
#      configs/polymarket.yaml -> logging.dir /data/syslog, storage /data/polymarket_2
#                                 (storage only if storage.enabled: true)
sudo mkdir -p /data/syslog /data/orderbook_2 /data/polymarket_2
sudo chown -R velociraptor:velociraptor /data/syslog /data/orderbook_2 /data/polymarket_2
# -R matters: a crash-looping recorder may have already created subdirs as
# root on earlier failed starts; a top-level chown alone won't fix those.

# 4. Install rust FOR the service user (one-time)
sudo -u velociraptor sh -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'

# 5. Build the release binaries — REQUIRED before first start; the units exec
#    target/release/<bin> directly and will not start without it.
#    Build AS the service user with a clean env (env -i): if you build from a
#    shell with conda/venv active, the binary can pick up that toolchain's
#    linker/libs and fail to run under the bare service account.
sudo -u velociraptor env -i HOME=/home/velociraptor PATH=/home/velociraptor/.cargo/bin:/usr/bin:/bin \
  bash -c 'cd /opt/velociraptor && cargo build --release'

# 6. Install the units
sudo cp /opt/velociraptor/deploy/systemd/*.service \
        /opt/velociraptor/deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload

# 7. Enable + start everything
sudo systemctl enable --now velociraptor.target
sudo systemctl enable --now \
  velociraptor-polymarket-recorder.service \
  velociraptor-orderbook-recorder.service \
  velociraptor-price-to-beat-fetcher.service \
  velociraptor-asset-id-fetcher.service

# 8. Verify all four are active (running)
systemctl status 'velociraptor-*.service' --no-pager
```

If a unit was left in `failed`/`start-limit-hit` from an earlier attempt,
clear it before retrying: `sudo systemctl reset-failed 'velociraptor-*.service'`.

## Operate

```bash
# Status (one / all)
systemctl status velociraptor-orderbook-recorder.service --no-pager -l
systemctl status 'velociraptor-*.service' --no-pager
systemctl is-active velociraptor-orderbook-recorder.service   # inactive|active|failed

# Restart one / all
sudo systemctl restart velociraptor-asset-id-fetcher.service
sudo systemctl restart velociraptor-*.service

# Stop NOW but still auto-start on reboot (use this for maintenance, e.g.
# fixing /data permissions — stop first so crash-looping doesn't recreate
# root-owned dirs while you chown)
sudo systemctl stop velociraptor-*.service

# Stop AND prevent reboot auto-start (full takedown)
sudo systemctl disable --now velociraptor-*.service
# …re-enable later with: sudo systemctl enable --now velociraptor-*.service
```

### Peeking at output

systemd captures stdout/stderr into the **journal** — there's no attached
terminal. Query it:

```bash
journalctl -u velociraptor-orderbook-recorder.service -f          # live tail
journalctl -u velociraptor-orderbook-recorder.service -n 50 --no-pager
journalctl -u velociraptor-orderbook-recorder.service -b --no-pager  # since last start
journalctl -u velociraptor-orderbook-recorder.service --since "10 min ago" --no-pager
# Several units interleaved:
journalctl -f -u velociraptor-orderbook-recorder.service -u velociraptor-asset-id-fetcher.service
```

**`journalctl` shows `-- No entries --` / "not seeing messages from other
users"?** The services run as `velociraptor`; a non-root login only sees its
own journal. Either prefix `sudo`, or grant your login account read access
once (takes effect on next login):

```bash
sudo usermod -aG systemd-journal <your-user>   # e.g. ben; then re-login
groups   # confirm: …systemd-journal…
```

**On-disk log files (recorders only).** `orderbook_recorder` and
`polymarket_recorder` also write rotating files under `{logging.dir}` — survives
journal rotation and isolates errors. Fastest way to see what's wrong:

```bash
sudo tail -f /data/syslog/orderbook_recorder/$(date -u +%F).log
sudo tail -f /data/syslog/orderbook_recorder/$(date -u +%F).error.log   # WARN+ only
```

The two fetchers log to stdout only → journald is the only source for them.

## Upgrade workflow (to-dos after this repo is updated)

**Use the script** — it does pull → build → re-sync units → restart → status,
in the order that guarantees no downtime, and stops hard if the build fails:

```bash
sudo /opt/velociraptor/deploy/systemd/update.sh
```

What it does, and why the order matters, if you'd rather run it by hand:

```bash
# 1. Pull (as the service user, into the deployed repo)
sudo -u velociraptor git -C /opt/velociraptor pull

# 2. Build FIRST — the OLD binaries keep running untouched during the compile.
#    Clean env (env -i) so an interactive conda/venv can't contaminate it.
sudo -u velociraptor env -i HOME=/home/velociraptor PATH=/home/velociraptor/.cargo/bin:/usr/bin:/bin \
  bash -c 'cd /opt/velociraptor && cargo build --release'

# 3. If a unit file changed in this update, re-install it (cheap to always do)
sudo cp /opt/velociraptor/deploy/systemd/*.service \
        /opt/velociraptor/deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload

# 4. ONLY if step 2 succeeded: swap to the new binaries (sub-second exec)
sudo systemctl restart velociraptor-*.service

# 5. Verify
systemctl status 'velociraptor-*.service' --no-pager
```

Why this ordering gives near-zero downtime: the running services keep using
the old `target/release/` binaries throughout the (possibly slow) build, and
the restart only happens once the new binaries are in place. **If the build
fails, do not restart** — the old binaries are untouched and the services keep
running on the old code. Fix the build, then retry. Restart is a sub-second
exec swap, not a recompile.

Things this update flow does **not** do automatically — handle these yourself
when the change calls for it:

- **Config schema changes.** New/renamed keys in `configs/*.yaml` aren't
  migrated. If your deployed configs differ from the repo's, reconcile them
  (and mind that `git pull` may conflict on locally-edited config files).
- **Log dir.** If `logging.dir` changes in a config, pre-create the new path
  and `chown velociraptor:velociraptor` it (see Install step 3) — otherwise
  the recorders panic on the next restart.
- **New/removed services.** If this repo adds or drops a unit file, enable the
  new one (`systemctl enable --now <unit>`) or stop+disable the removed one;
  step 3 only refreshes existing units.

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

## Emergency error handling (fast runbook)

A service is failing — triage in this order:

```bash
# 1. What state is it in, and what was the last failure?
systemctl status velociraptor-orderbook-recorder.service --no-pager -l
#    Read the `Process:` / `Active:` lines: 217/USER, 203/EXEC, start-limit-hit,
#    or "active (running)" but misbehaving.

# 2. Why did the process itself exit? (skip if it never got to ExecStart)
journalctl -u velociraptor-orderbook-recorder.service -n 80 --no-pager
sudo tail -n 80 /data/syslog/orderbook_recorder/$(date -u +%F).error.log  # recorders

# 3. Stop the bleed if it's crash-looping (prevents root-owned dirs being
#    recreated while you fix /data perms, and stops log spam):
sudo systemctl stop velociraptor-orderbook-recorder.service

# 4. Fix the root cause (see table below), then clear the failed state and start:
sudo systemctl reset-failed velociraptor-orderbook-recorder.service
sudo systemctl start velociraptor-orderbook-recorder.service

# 5. Confirm it stays up (watch for ~30s; a crash-looper will flip back to
#    activating/failed):
watch -n2 "systemctl is-active velociraptor-orderbook-recorder.service"
```

`reset-failed` is mandatory after `start-limit-hit` — until you run it,
`start` is refused. Stop *before* touching `/data` ownership: a crash-looping
recorder recreates dirs as root between your `chown` and its next restart.

Whole-fleet emergency stop (one command):

```bash
sudo systemctl stop velociraptor-*.service
```

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `StorageWriter: failed to open /data/<...> : Permission denied (os error 13)` | `storage.base_path` dir not writable by the service user. `sudo mkdir -p <path> && sudo chown -R velociraptor:velociraptor <path>`, then restart. Distinct from the log-dir panic — this is per-file and the service stays up but records nothing. (Install step 3.) |
| `status=200/CHDIR` | `WorkingDirectory=/opt/velociraptor` doesn't exist or isn't readable by the service user. |
| `ExecStartPre` failed / `203/EXEC` | Release binary not built. Run `cargo build --release` (the `target/release/<bin>` is missing or not executable). |
| `217/USER` (on **all** units) | The `velociraptor` user/group doesn't exist — Install step 1 skipped. systemd can't resolve `User=`, so `ExecStartPre` never even runs. Create the user, then `reset-failed` + start. This is the #1 first-deploy failure. |
| Recorder exits immediately, log mentions creating `/data/syslog` | Log dir missing or not writable by the service user → `init_logging` panics on `create_dir_all`. `sudo mkdir -p /data/syslog && sudo chown velociraptor:velociraptor /data/syslog` (Install step 3). |
| Binary built fine but won't run / linker or libc errors | Built from a shell with conda/venv active, contaminating the toolchain. Rebuild with the clean-env command (Install step 5 / `update.sh`). |
| `failed (Result: start-limit-hit)` | Binary crash-looping on startup (bad config, unreachable Redis, panic) or unbuilt. Inspect `journalctl -u <unit>`, fix the cause, then `reset-failed`. |
| Starts then exits 0 immediately | A *fetcher* may be a run-once job, not a daemon. If so, `Restart=always` will keep relaunching it on `RestartSec=5` — switch that unit to `Restart=on-failure`, or run it as a `systemd.timer` instead of an always-on service. Confirm the intended runtime model before deploying. |
| Config not found | `--config configs/...` is relative to `WorkingDirectory`. Confirm the YAML exists at `/opt/velociraptor/configs/`. |

Inspect a failing unit:

```bash
systemctl status velociraptor-asset-id-fetcher.service
journalctl -u velociraptor-asset-id-fetcher.service -n 100 --no-pager
sudo systemd-analyze verify /etc/systemd/system/velociraptor-asset-id-fetcher.service
```
