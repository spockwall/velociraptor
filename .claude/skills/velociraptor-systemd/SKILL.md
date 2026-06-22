---
name: velociraptor-systemd
description: Deploying the four long-running velociraptor binaries (polymarket_recorder, orderbook_recorder, price_to_beat_fetcher, asset_id_fetcher) as Linux systemd services running as user `ben` from /home/ben/velociraptor — unit layout, the pre-built-binary model, install/upgrade order, and the failure modes that bite in practice (/data permission denials, conda-contaminated builds, start-limit-hit). Use when deploying, debugging, or modifying the systemd units under deploy/systemd/.
---

# Velociraptor — systemd deployment

Runs the four long-running binaries as Linux systemd services, **as user
`ben`** from `/home/ben/velociraptor` (no dedicated service account — maintained
as the normal login user). Units, the `update.sh` helper, and a long-form
README live in `deploy/systemd/`. This skill is the distilled operational
knowledge; the README is the full reference.

> Linux only — the dev machine is macOS (no systemd).

## The four units

| Unit | Binary | Config |
|---|---|---|
| `velociraptor-polymarket-recorder.service` | `polymarket_recorder` | `configs/prod/polymarket.yaml` |
| `velociraptor-orderbook-recorder.service` | `orderbook_recorder` | `configs/prod/recorder.yaml` |
| `velociraptor-price-to-beat-fetcher.service` | `price_to_beat_fetcher` | `configs/prod/recorder.yaml` |
| `velociraptor-asset-id-fetcher.service` | `asset_id_fetcher` | `configs/prod/recorder.yaml` |

`velociraptor.target` groups all four (it uses `Wants=`; the services are
pulled into boot by their own `WantedBy=multi-user.target`, *not* by the
target — enabling the four services is what makes them reboot-persistent).

## Key design decisions (and why)

- **Runs as `ben`.** `User=ben`, `WorkingDirectory=/home/ben/velociraptor`.
  No `velociraptor` service account — `git pull` / `cargo build` / log reading
  need no `sudo -u`. (An earlier design used a dedicated `velociraptor` user;
  dropped as too much maintenance overhead for a single-operator box.)
- **Pre-built binaries, not `cargo run`.**
  `ExecStart=/home/ben/velociraptor/target/release/<bin>`. Restart is a
  sub-second exec swap; a broken build can never take a running service down
  (you just don't restart until the build is green). Trade-off: **you must
  `cargo build --release` yourself** — nothing rebuilds automatically.
- **`ExecStartPre=/usr/bin/test -x <binary>`** on every unit — a missing build
  fails fast with a clear error instead of a cryptic `203/EXEC`.
- **`KillSignal=SIGINT`**, `TimeoutStopSec=30` — recorders need `SIGINT`
  (not `SIGTERM`) to flush their MessagePack buffer and close files cleanly.
- **`Restart=always`, `RestartSec=5`** — survive crashes; 5 s backoff avoids
  CPU spin in a tight crash loop.
- **Configs read once at startup** — no file-watch / `SIGHUP` / `ExecReload`.
  A config edit applies only on `systemctl restart`.

## Hardcoded in the units (edit them if the machine differs)

1. User/group: **`ben`**
2. Repo / `WorkingDirectory`: **`/home/ben/velociraptor`**
3. Binaries: **`/home/ben/velociraptor/target/release/<name>`**

## Install order (first-time)

The repo is already at `/home/ben/velociraptor` and you build as yourself, so
setup is short:

1. **Pre-create every `/data` path the configs write to**, `chown -R ben:ben`
   them (see "The /data permission trap")
2. `cargo build --release` **with a clean env** (`env -i`, see "conda
   contamination")
3. `sudo cp deploy/systemd/*.service *.target /etc/systemd/system/` +
   `sudo systemctl daemon-reload`
4. `sudo systemctl enable --now` the target and the four services, then
   `systemctl status 'velociraptor-*.service'` to verify

## Upgrade order (after a repo update)

**Use the script:** `/home/ben/velociraptor/deploy/systemd/update.sh`

Order is load-bearing: **build → (only on success) restart**. The running
services keep using the *old* `target/release/` binaries throughout the (slow)
build, so a slow or failing build never causes downtime. If the build fails,
do **not** restart — old binaries keep running on old code; fix and retry.
`update.sh` does this plus re-syncs unit files and stops hard on build failure
(`set -euo pipefail`). It needs `sudo` only for the unit-file copy and
`systemctl`, not for build.

`update.sh` deliberately does **not** run `git pull` — pull/merge happens by
hand so a local config edit (e.g. enabled markets, `storage.base_path`) can't
be silently clobbered. Workflow is: `git pull` (resolve any config conflicts)
→ `update.sh`.

Not automated — handle manually: `git pull` itself plus any conflict
reconciliation on edited configs, a changed
`logging.dir`/`storage.base_path`/`fetcher.*_dir` needing a fresh `chown`, and
added/removed units needing manual `enable`/`disable`.

## Failure modes seen in practice (highest-value section)

### 1. The /data permission trap (the recurring one)
`/data` is root-owned; `ben` can't write there until chowned. **Three**
distinct symptoms from three distinct config keys:

- **`{logging.dir}`** (e.g. `/data/syslog`) — both recorders call
  `init_logging` → `create_dir_all` and **panic on startup** if it fails. The
  service won't stay up.
- **`{storage.base_path}`** (`/data/orderbook` for `orderbook_recorder`,
  `/data/polymarket` for `polymarket_recorder`) — `orderbook_recorder` logs
  `StorageWriter: failed to open /data/... : Permission denied (os error 13)`
  *per file*; the service stays up but records nothing.
- **`{fetcher.asset_id_dir}` / `{fetcher.price_to_beat_dir}`** (e.g.
  `/data/asset_ids`, `/data/price_to_beat`) — the fetchers can't write their
  per-day CSVs; no archive is produced.

Fix all: `sudo mkdir -p <path> && sudo chown -R ben:ben <path>`. **`-R` is
mandatory** — a crash-looping recorder may already have created subdirs as
root on earlier failed starts. **Stop the service before chowning** —
otherwise it recreates root-owned dirs between your `chown` and its next
restart. The prod configs and the paths they write:
- `configs/prod/recorder.yaml` (orderbook_recorder + both fetchers):
  `logging.dir` → `/data/syslog`, `storage.base_path` → `/data/orderbook`,
  `fetcher.asset_id_dir` → `/data/asset_ids`, `fetcher.price_to_beat_dir` → `/data/price_to_beat`
- `configs/prod/polymarket.yaml` (polymarket_recorder):
  `logging.dir` → `/data/syslog`, `storage.base_path` → `/data/polymarket`
  (files land at `/data/polymarket/{slug}/{date}/…`)

The fetcher archive dir resolves as: `--archive-dir` CLI flag (if passed) >
`fetcher.{asset_id,price_to_beat}_dir` in the config > built-in default
(`./data/asset_ids` / `./data/price_to_beat`, relative to `WorkingDirectory`).
The systemd units pass no `--archive-dir`, so the config value wins.

### 2. conda/venv-contaminated build
The `ben` shell often has conda active (prompt `(base)`). Building from it can
bake that toolchain's linker/libs into the binary, which then misbehaves at
runtime. Always build with a clean env:
```bash
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  bash -c 'cd /home/ben/velociraptor && cargo build --release'
```

### 3. `start-limit-hit`
Binary crash-loops on startup (bad config, unreachable Redis, panic, or
unbuilt). systemd's default limit is 5 starts / 10 s. **`reset-failed` is
mandatory** before `start` will be accepted again — fix the root cause first.
To soften boot recovery, add `StartLimitIntervalSec=0` to `[Service]`.

### 4. Fetcher exits immediately, `inactive`
Log says `no enabled … markets` → `configs/prod/recorder.yaml` has no enabled
markets or the file is missing. The fetchers are genuine daemons (infinite
poll loop after backfill) — any clean exit is an error path. Fix config,
restart.

### 5. `status=217/USER` on all units
The units' `User=ben` can't be resolved. Confirm `id ben`, or edit
`User=`/`Group=` in the unit files. (Was the #1 failure under the old
dedicated-user design; unlikely now since `ben` is the login user.)

## Emergency runbook

```bash
# Triage (ben can read its own services' journal — no sudo)
systemctl status velociraptor-orderbook-recorder.service --no-pager -l
journalctl -u velociraptor-orderbook-recorder.service -n 80 --no-pager
tail -n 80 /data/syslog/orderbook_recorder/$(date -u +%F).error.log  # recorders only

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

Services run as `ben` (the login user), so `journalctl` needs no `sudo`.
Fastest error view for a recorder:
`tail -f /data/syslog/<service>/$(date -u +%F).error.log`

## Related

- `deploy/systemd/README.md` — full reference (this skill is the summary)
- `deploy/systemd/update.sh` — the upgrade automation
- `mcp_monitor/` — sidecar MCP server (`velociraptor-mcp-monitor.service`)
  that exposes these units' health + system CPU/RAM as MCP tools for Claude
  Code. Read-only plus a single allowlisted `restart_unit` mutation.
- `velociraptor-storage` skill — the MessagePack files the recorders write
- `velociraptor-overview` skill — workspace/crate map
