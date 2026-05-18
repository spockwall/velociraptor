#!/usr/bin/env bash
# Update the deployed velociraptor services to the latest code.
#
#   sudo /opt/velociraptor/deploy/systemd/update.sh
#
# Order matters: pull → build → (only on success) restart. The running
# services keep using the OLD target/release/ binaries throughout the build,
# so a slow or failing build never takes anything down — we only restart
# once the new binaries are in place.
set -euo pipefail

SERVICE_USER="velociraptor"
REPO="/opt/velociraptor"
UNITS=(
  velociraptor-polymarket-recorder.service
  velociraptor-orderbook-recorder.service
  velociraptor-price-to-beat-fetcher.service
  velociraptor-asset-id-fetcher.service
)

if [[ $EUID -ne 0 ]]; then
  echo "Run with sudo (needs systemctl + -u $SERVICE_USER)." >&2
  exit 1
fi

echo "==> git pull (as $SERVICE_USER)"
sudo -u "$SERVICE_USER" git -C "$REPO" pull

# Build as the service user with a clean env. Do NOT inherit an active conda
# / virtualenv from an interactive shell — it can contaminate the linker and
# produce a binary that won't run under the bare service account.
echo "==> cargo build --release (as $SERVICE_USER, clean env)"
sudo -u "$SERVICE_USER" env -i HOME="/home/$SERVICE_USER" PATH="/home/$SERVICE_USER/.cargo/bin:/usr/bin:/bin" \
  bash -c "cd '$REPO' && cargo build --release"

# If a unit file changed, re-install it. Cheap to always do.
echo "==> sync unit files"
cp "$REPO"/deploy/systemd/*.service "$REPO"/deploy/systemd/*.target /etc/systemd/system/
systemctl daemon-reload

echo "==> restart services (sub-second exec swap)"
systemctl restart "${UNITS[@]}"

echo "==> status"
systemctl --no-pager --no-legend status "${UNITS[@]}" | grep -E 'velociraptor-|Active:' || true
echo "Done. Tail logs with: journalctl -u <unit> -f"
