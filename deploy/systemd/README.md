# Velociraptor systemd units

Runs the four long-running binaries as Linux systemd services, **as user
`ben`** from `/home/ben/velociraptor`. Linux only (the dev machine is macOS —
no systemd).

| Unit | Binary | Config |
|---|---|---|
| `velociraptor-polymarket-recorder.service` | `polymarket_recorder` | `configs/prod/polymarket.yaml` |
| `velociraptor-orderbook-recorder.service` | `orderbook_recorder` | `configs/prod/recorder.yaml` |
| `velociraptor-price-to-beat-fetcher.service` | `price_to_beat_fetcher` | `configs/prod/recorder.yaml` |
| `velociraptor-asset-id-fetcher.service` | `asset_id_fetcher` | `configs/prod/recorder.yaml` |

`velociraptor.target` groups all four. Binaries are pre-built and run from
`/home/ben/velociraptor/target/release/` — **not** `cargo run`.

---

## Quick reference

```bash
# Status
systemctl status 'velociraptor-*.service' --no-pager
systemctl is-active velociraptor-asset-id-fetcher.service   # active|inactive|failed

# Logs (ben can read its own services' logs — no sudo needed)
journalctl -u velociraptor-asset-id-fetcher.service -f
journalctl -u velociraptor-asset-id-fetcher.service -n 80 --no-pager

# Restart / stop
sudo systemctl restart velociraptor-*.service
sudo systemctl stop    velociraptor-*.service          # stays enabled for reboot

# Apply a config change → must restart (configs are read once at startup)
sudo systemctl restart velociraptor-asset-id-fetcher.service

# Deploy a code update
/home/ben/velociraptor/deploy/systemd/update.sh

# A unit is "failed" and won't start → clear the latch first
sudo systemctl reset-failed velociraptor-asset-id-fetcher.service
sudo systemctl start       velociraptor-asset-id-fetcher.service
```

**First-time setup?** → [Install](#install). **Something broke?** →
[Diagnosing a failure](#diagnosing-a-failure).

---

## How it works

- **Runs as `ben`.** `User=ben`, `WorkingDirectory=/home/ben/velociraptor`. No
  dedicated service account — you maintain it as your normal login user, so
  `git pull` / `cargo build` / log reading need no `sudo -u`.
- **Pre-built binaries, not `cargo run`.** `ExecStart` points at
  `/home/ben/velociraptor/target/release/<bin>`. Restart is a sub-second exec
  swap; a broken build can't take a running service down. Trade-off: **you
  must `cargo build --release` yourself** — nothing rebuilds automatically.
- **`ExecStartPre=test -x <binary>`** — a missing build fails fast with a clear
  error instead of a cryptic `203/EXEC`.
- **`Restart=always`, `RestartSec=5`** — survive crashes; 5 s backoff.
- **`KillSignal=SIGINT`, `TimeoutStopSec=30`** — recorders need `SIGINT` (not
  `SIGTERM`) to flush the MessagePack buffer and close files cleanly.
- **`WantedBy=multi-user.target`** — `systemctl enable` wires the unit into
  boot. The `.target` only *groups* (it uses `Wants=`); reboot persistence
  comes from enabling the **services**, not the target.
- **Configs are read once at process startup.** No file-watch, no `SIGHUP`, no
  `ExecReload`. A config edit takes effect only on `systemctl restart`.

**Hardcoded in the units** — edit them if your machine differs: user/group
`ben`, repo at `/home/ben/velociraptor`, binaries at
`/home/ben/velociraptor/target/release/`.

---

## Install

The repo is already at `/home/ben/velociraptor` and you build as yourself, so
setup is short.

```bash
# 1. Pre-create EVERY /data path the configs write to, owned by ben.
#    /data is root-owned; ben can't write there until chowned.
#      logging.dir            -> recorders PANIC on startup if missing
#      storage.base_path      -> orderbook_recorder errors per-file, stays up
#      fetcher.{asset_id,price_to_beat}_dir -> fetchers can't write CSVs
#    Paths the prod configs write to:
#      configs/prod/recorder.yaml   = /data/syslog + /data/orderbook
#                                     + /data/asset_ids + /data/price_to_beat
#      configs/prod/polymarket.yaml = /data/syslog + /data/polymarket
sudo mkdir -p /data/syslog /data/orderbook /data/polymarket /data/asset_ids /data/price_to_beat
sudo chown -R ben:ben \
  /data/syslog /data/orderbook /data/polymarket /data/asset_ids /data/price_to_beat

# 2. Build (clean env — an active conda/venv can contaminate the binary)
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  bash -c 'cd /home/ben/velociraptor && cargo build --release'

# 3. Install + enable + start
sudo cp /home/ben/velociraptor/deploy/systemd/*.service \
        /home/ben/velociraptor/deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now velociraptor.target
sudo systemctl enable --now \
  velociraptor-polymarket-recorder.service \
  velociraptor-orderbook-recorder.service \
  velociraptor-price-to-beat-fetcher.service \
  velociraptor-asset-id-fetcher.service

# 4. Verify
systemctl status 'velociraptor-*.service' --no-pager
```

---

## Update (after a repo change)

```bash
/home/ben/velociraptor/deploy/systemd/update.sh
```

The script does **build → re-sync units → restart → status**, and stops hard
if the build fails (`set -euo pipefail`). It deliberately does **not** touch
git — `git pull` (and any config-merge reconciliation) is done by hand so a
local config edit can't be clobbered. The ordering is the point: running
services keep using the *old* binaries throughout the (slow) build, so a
failing build causes **zero downtime** — you simply don't reach the restart.
Equivalent by hand:

```bash
git -C /home/ben/velociraptor pull                               # manual, separate step
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  bash -c 'cd /home/ben/velociraptor && cargo build --release'   # build FIRST
sudo cp /home/ben/velociraptor/deploy/systemd/*.service \
        /home/ben/velociraptor/deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl restart velociraptor-*.service                    # only if build OK
systemctl status 'velociraptor-*.service' --no-pager
```

Build with the `env -i` clean-env wrapper: your `ben` shell often has conda
(`(base)`) active, which can bake the wrong toolchain/libs into the binary.

**Not automated** — do these by hand when the change calls for it:

- **Pulling code.** `update.sh` doesn't run `git pull`. Pull yourself before
  running it, and reconcile any conflicts on locally-edited `configs/*.yaml`
  (configs only reload on `systemctl restart`).
- **New `logging.dir` / `storage.base_path` / `fetcher.*_dir`.** Pre-create +
  `chown ben:ben` the new path before restarting, or the service fails.
- **Added/removed units.** Manually `enable --now` a new unit or
  `disable --now` a removed one; the update only refreshes existing files.

---

## Diagnosing a failure

Triage in this order:

```bash
# 1. State + last failure reason
systemctl status velociraptor-<unit>.service --no-pager -l
#    Look at the Process: / Active: lines.

# 2. Why the process exited (if it got past ExecStartPre)
journalctl -u velociraptor-<unit>.service -n 80 --no-pager
tail -n 80 /data/syslog/<service>/$(date -u +%F).error.log   # recorders only

# 3. Fix the cause (table below), then clear the failed latch and start
sudo systemctl reset-failed velociraptor-<unit>.service
sudo systemctl start       velociraptor-<unit>.service
```

`reset-failed` is **mandatory** once a unit is `failed` /
`start-limit-hit` — systemd refuses `start` until the latch is cleared.
(`start-limit-hit` = crashed 5+ times in 10 s; to soften, add
`StartLimitIntervalSec=0` to the unit's `[Service]` section.)

When fixing `/data` ownership, **`stop` the service first** — a crash-looping
process recreates root-owned dirs between your `chown` and its next restart.
Use `-R` on the chown to fix dirs an earlier crash already made as root.

### Failure table

| Symptom (in `status` / `journalctl`) | Cause → fix |
|---|---|
| `status=217/USER` on **all** units | The units' `User=` doesn't exist. They expect `ben`; confirm `id ben` resolves, or edit `User=`/`Group=` in the unit files. |
| `ExecStartPre` failed / `203/EXEC` | Binary not built. Run the clean-env `cargo build --release` (Install step 2). |
| `status=200/CHDIR` | `/home/ben/velociraptor` missing or unreadable. |
| Recorder exits at once; log mentions creating `/data/syslog` | `logging.dir` not writable → `init_logging` panics. `mkdir -p` + `chown -R ben:ben` it. |
| `StorageWriter: failed to open /data/… : Permission denied (os error 13)` | `storage.base_path` not writable. `mkdir -p` + `chown -R ben:ben`. Service stays up but records nothing. |
| Built fine but won't run / linker or libc errors | Built with conda/venv active. Rebuild with the `env -i` clean-env command. |
| Fetcher: exits immediately, `inactive`, log says `no enabled … markets` | `configs/prod/recorder.yaml` has no enabled markets, or the file is missing. Fix the config, restart. |
| `failed (Result: start-limit-hit)` | Crash-looping (bad config, unreachable Redis, panic) or unbuilt. Inspect journal, fix, `reset-failed`. |
| Config not found | `--config configs/…` is relative to `/home/ben/velociraptor`. Confirm the YAML exists there. |

Validate a unit file itself:

```bash
sudo systemd-analyze verify /etc/systemd/system/velociraptor-asset-id-fetcher.service
```

---

## Logs

| | Recorders (`orderbook`, `polymarket`) | Fetchers (`price_to_beat`, `asset_id`) |
|---|---|---|
| journald (`journalctl`) | ✅ mirrored | ✅ only source |
| On-disk files | ✅ `{logging.dir}/<service>/{date}.log` + `.error.log` (WARN+), daily-rotated | ❌ |

```bash
journalctl -u velociraptor-<unit>.service -f
tail -f /data/syslog/<recorder>/$(date -u +%F).error.log   # recorders, errors only
```

Services run as `ben`, your login user, so `journalctl` works without `sudo`.
(If you log in as a *different* user and see `-- No entries --`, that user
needs `sudo` or membership in the `systemd-journal` group.)
