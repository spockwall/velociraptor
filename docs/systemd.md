# Running as systemd Services

Two service files are provided under `systemd/`:

| File | Binary | Config |
|------|--------|--------|
| `orderbook-server.service` | `orderbook_server` | `configs/server.toml` |
| `polymarket-recorder.service` | `polymarket_recorder` | `configs/polymarket.toml` |

Both are designed for continuous, unattended operation (>1 year uptime).

---

## Prerequisites

- Linux host with systemd
- Rust toolchain installed (the service files reference `/Users/spockwall/.cargo/bin/cargo` — **edit `User=` and all absolute paths** to match your deployment host before installing)
- The repo checked out at a stable path (the `WorkingDirectory=` and binary paths are hardcoded)

---

## Installation

### 1. Edit paths for your host

Open each `.service` file and update:

```ini
User=<your-unix-user>
WorkingDirectory=/path/to/velociraptor
ExecStartPre=/home/<user>/.cargo/bin/cargo build ...
ExecStart=/path/to/velociraptor/target/release/<binary> ...
```

### 2. Build the release binaries

```bash
cargo build --bin orderbook_server --release
cargo build --bin polymarket_recorder --release
```

If you keep `ExecStartPre=` in the service file, the build runs automatically at each service start. Remove it to skip the build step and use the pre-built binary directly.

### 3. Install and enable

```bash
sudo cp systemd/orderbook-server.service    /etc/systemd/system/
sudo cp systemd/polymarket-recorder.service /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable --now orderbook-server
sudo systemctl enable --now polymarket-recorder
```

`enable --now` both starts the service immediately and registers it to start on every boot.

---

## Managing the Services

```bash
# Status
sudo systemctl status orderbook-server
sudo systemctl status polymarket-recorder

# Follow live logs
sudo journalctl -u orderbook-server -f
sudo journalctl -u polymarket-recorder -f

# Historical logs (last 100 lines)
sudo journalctl -u orderbook-server -n 100 --no-pager
sudo journalctl -u polymarket-recorder -n 100 --no-pager

# Stop / start / restart manually
sudo systemctl stop    orderbook-server
sudo systemctl start   orderbook-server
sudo systemctl restart orderbook-server

# Disable autostart
sudo systemctl disable orderbook-server
```

---

## Restart Policy

Both services use the same restart policy, tuned for long-running production use:

| Setting | Value | Effect |
|---------|-------|--------|
| `Restart=always` | always | Restarts on any exit — crash, OOM, or clean exit |
| `RestartSec=10s` | 10 s | Waits 10 s before each restart, preventing tight crash loops |
| `StartLimitBurst=10` | 10 attempts | Allows up to 10 rapid restarts… |
| `StartLimitIntervalSec=300` | 5 min window | …within a 5-minute window before systemd gives up |

If systemd stops retrying (hit the burst limit), restart manually and investigate logs:

```bash
sudo systemctl reset-failed orderbook-server
sudo systemctl start orderbook-server
```

---

## File Descriptor Limit

Both services set `LimitNOFILE=65536` to accommodate many concurrent WebSocket connections. If you run into `Too many open files` errors with a large number of symbols, raise this value.

---

## Disk Space (Polymarket Recorder)

The recorder writes continuously to `base_path` defined in `configs/polymarket.toml` (default `/data/polymarket`). Monitor free space on the target volume — there is no built-in eviction. A simple cron guard:

```bash
# /etc/cron.d/polymarket-disk-check
*/15 * * * * root df /data/polymarket | awk 'NR==2{if($5+0>90) system("systemctl stop polymarket-recorder")}'
```

---

## Updating the Binary

```bash
# 1. Pull latest code
git pull

# 2. Rebuild
cargo build --bin orderbook_server --release
cargo build --bin polymarket_recorder --release

# 3. Restart services to pick up the new binary
sudo systemctl restart orderbook-server
sudo systemctl restart polymarket-recorder
```
