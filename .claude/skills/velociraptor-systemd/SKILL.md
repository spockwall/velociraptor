---
name: velociraptor-systemd
description: Deploying the four long-running velociraptor binaries (polymarket_recorder, orderbook_recorder, price_to_beat_fetcher, asset_id_fetcher) as Linux systemd services — unit layout, the pre-built-binary model, install/upgrade order, and the failure modes that bite in practice (217/USER, /data permission denials, conda-contaminated builds, journal access). Use when deploying, debugging, or modifying the systemd units under deploy/systemd/.
---

# Velociraptor — systemd deployment

Runs the four long-running binaries as Linux systemd services. Units, the
`update.sh` helper, and a long-form README live in `deploy/systemd/`. This
skill is the distilled operational knowledge; the README is the full reference.

> Linux only — the dev machine is macOS (no systemd). The units are deployed
> to a Linux server.

## The four units

| Unit | Binary | Config |
|---|---|---|
| `velociraptor-polymarket-recorder.service` | `polymarket_recorder` | `configs/polymarket.yaml` |
| `velociraptor-orderbook-recorder.service` | `orderbook_recorder` | `configs/server.yaml` |
| `velociraptor-price-to-beat-fetcher.service` | `price_to_beat_fetcher` | `configs/fetcher.yaml` |
| `velociraptor-asset-id-fetcher.service` | `asset_id_fetcher` | `configs/fetcher.yaml` |

`velociraptor.target` groups all four (it uses `Wants=`; the services are
pulled into boot by their own `WantedBy=multi-user.target`, *not* by the
target — enabling the four services is what makes them reboot-persistent).

## Key design decisions (and why)

- **Pre-built binaries, not `cargo run`.** `ExecStart=/opt/velociraptor/target/release/<bin>`,
  not `cargo run`. Restart is a sub-second exec swap; a broken build can never
  take a running service down (you just don't restart until the build is
  green). Trade-off: **you must `cargo build --release` yourself** before the
  first start and after every code change — nothing rebuilds automatically.
- **`ExecStartPre=/usr/bin/test -x <binary>`** on every unit — a missing build
  fails fast with a clear error instead of a cryptic `203/EXEC`.
- **`KillSignal=SIGINT`**, `TimeoutStopSec=30` — recorders need `SIGINT`
  (not `SIGTERM`) to flush their MessagePack buffer and close files cleanly.
- **`Restart=always`, `RestartSec=5`** — survive crashes; 5 s backoff avoids
  CPU spin in a tight crash loop.

## Three hardcoded assumptions (edit units if they differ)

1. Service user/group: **`velociraptor`**
2. Repo / `WorkingDirectory`: **`/opt/velociraptor`**
3. Binaries: **`/opt/velociraptor/target/release/<name>`**

## Install order (first-time)

1. `useradd --system --create-home --shell /usr/sbin/nologin velociraptor`
2. Repo to `/opt/velociraptor` (clone, or `cp -a` an existing clone to keep
   edited configs), then `chown -R velociraptor:velociraptor /opt/velociraptor`
3. **Pre-create every `/data` path the services write to**, `chown -R` them to
   `velociraptor` (see "The /data permission trap" below)
4. Install rust *for* the `velociraptor` user (rustup)
5. `cargo build --release` **as the velociraptor user with a clean env**
   (`env -i`, see "conda contamination" below)
6. `cp deploy/systemd/*.service *.target /etc/systemd/system/` + `daemon-reload`
7. `systemctl enable --now` the target and the four services
8. `systemctl status 'velociraptor-*.service'` to verify

## Upgrade order (after a repo update)

**Use the script:** `sudo /opt/velociraptor/deploy/systemd/update.sh`

Order is load-bearing: **pull → build → (only on success) restart**. The
running services keep using the *old* `target/release/` binaries throughout the
(slow) build, so a slow or failing build never causes downtime. If the build
fails, do **not** restart — old binaries keep running on old code; fix and
retry. `update.sh` does this plus re-syncs unit files and stops hard on build
failure (`set -euo pipefail`).

Not automated — handle manually: config schema drift / `git pull` conflicts on
edited configs, a changed `logging.dir`/`storage.base_path` needing a fresh
`chown`, and added/removed units needing manual `enable`/`disable`.

## Failure modes seen in practice (highest-value section)

### 1. `status=217/USER` on *all four* units
The `velociraptor` user/group doesn't exist (install step 1 skipped). systemd
can't resolve `User=`, so `ExecStartPre` never even runs. **#1 first-deploy
failure.** Fix: create the user, `reset-failed`, start.

### 2. The /data permission trap
`/data` is root-owned; the unprivileged service user can't write there. Two
*distinct* symptoms from two *distinct* config keys:

- **`{logging.dir}`** (e.g. `/data/syslog`) — both recorders call
  `init_logging` → `create_dir_all` and **panic on startup** if it fails. The
  service won't stay up.
- **`{storage.base_path}`** (e.g. `/data/orderbook_2`, `/data/polymarket_2`) —
  `orderbook_recorder` logs `StorageWriter: failed to open /data/... :
  Permission denied (os error 13)` *per file*; the service stays up but
  records nothing.

Fix both: `sudo mkdir -p <path> && sudo chown -R velociraptor:velociraptor
<path>`. **`-R` is mandatory** — a crash-looping recorder may already have
created subdirs as root on earlier failed starts. **Stop the service before
chowning** — otherwise it recreates root-owned dirs between your `chown` and
its next restart. Paths come from the configs:
- `configs/server.yaml` → `logging.dir` `/data/syslog`, `storage.base_path` `/data/orderbook_2`
- `configs/polymarket.yaml` → `logging.dir` `/data/syslog`, `storage.base_path` `/data/polymarket_2` (storage only if `storage.enabled: true`)

### 3. conda/venv-contaminated build
Building from a shell with conda/venv active (prompt shows `(base)`) can bake
that toolchain's linker/libs into the binary, which then fails to run under the
bare service account. Always build with a clean env:
```bash
sudo -u velociraptor env -i HOME=/home/velociraptor PATH=/home/velociraptor/.cargo/bin:/usr/bin:/bin \
  bash -c 'cd /opt/velociraptor && cargo build --release'
```

### 4. `start-limit-hit`
Binary crash-loops on startup (bad config, unreachable Redis, panic, or
unbuilt). systemd's default limit is 5 starts / 10 s. **`reset-failed` is
mandatory** before `start` will be accepted again — fix the root cause first.
To soften boot recovery, add `StartLimitIntervalSec=0` to `[Service]`.

### 5. `journalctl` shows `-- No entries --`
Services run as `velociraptor`; a non-root login only sees its own journal.
Either prefix `sudo`, or `sudo usermod -aG systemd-journal <user>` (takes
effect on next login).

## Emergency runbook

```bash
# Triage
systemctl status velociraptor-orderbook-recorder.service --no-pager -l
journalctl -u velociraptor-orderbook-recorder.service -n 80 --no-pager
sudo tail -n 80 /data/syslog/orderbook_recorder/$(date -u +%F).error.log  # recorders only

# Stop the bleed (before fixing /data perms)
sudo systemctl stop velociraptor-orderbook-recorder.service     # one
sudo systemctl stop velociraptor-*.service                      # fleet

# Fix root cause, then:
sudo systemctl reset-failed velociraptor-orderbook-recorder.service
sudo systemctl start velociraptor-orderbook-recorder.service
```

`stop` pauses but keeps reboot auto-start; `disable --now` is a full takedown.

## Logging surfaces

- **Recorders** (`orderbook_recorder`, `polymarket_recorder`) use
  `libs::logging` → rotating files at `{logging.dir}/{service}/{YYYY-MM-DD}.log`
  + `.error.log` (WARN+), **and** mirrored to stdout (journald). Daily rotation
  is hardcoded in `libs/src/logging.rs` (`Rotation::DAILY`), not configurable.
- **Fetchers** (`price_to_beat_fetcher`, `asset_id_fetcher`) log to **stdout
  only** — journald is the only source.

Fastest error view for a recorder:
`sudo tail -f /data/syslog/<service>/$(date -u +%F).error.log`

## Related

- `deploy/systemd/README.md` — full reference (this skill is the summary)
- `deploy/systemd/update.sh` — the upgrade automation
- `velociraptor-storage` skill — the MessagePack files the recorders write
- `velociraptor-overview` skill — workspace/crate map
