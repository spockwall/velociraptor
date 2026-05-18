#!/usr/bin/env bash
# Update the deployed velociraptor services to the current source tree.
#
#   /home/ben/velociraptor/deploy/systemd/update.sh
#
# This does NOT touch git. Pull / merge / configs are managed by hand so a
# config edit isn't clobbered. Order matters: build → (only on success)
# re-sync units → restart. The running services keep using the OLD
# target/release/ binaries throughout the build, so a slow or failing build
# never takes anything down — we only restart once the new binaries are in
# place.
set -euo pipefail

REPO="/home/ben/velociraptor"
UNITS=(
  velociraptor-polymarket-recorder.service
  velociraptor-orderbook-recorder.service
  velociraptor-price-to-beat-fetcher.service
  velociraptor-asset-id-fetcher.service
)

# Build with a CLEAN env. Do NOT inherit an active conda / virtualenv from an
# interactive shell — it can contaminate the linker and produce a binary that
# misbehaves at runtime. (cargo lives under ~/.cargo/bin.)
echo "==> cargo build --release (clean env)"
env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
  bash -c "cd '$REPO' && cargo build --release"

# If a unit file changed, re-install it. Cheap to always do. Needs root.
echo "==> sync unit files"
sudo cp "$REPO"/deploy/systemd/*.service "$REPO"/deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload

echo "==> restart services (sub-second exec swap)"
sudo systemctl restart "${UNITS[@]}"

echo "==> status"
sudo systemctl --no-pager --no-legend status "${UNITS[@]}" | grep -E 'velociraptor-|Active:' || true
echo "Done. Tail logs with: journalctl -u <unit> -f"
