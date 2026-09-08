#!/usr/bin/env bash
# Install RustProxy systemd units and provision the runtime user/directories.
# Copies unit templates into the systemd search path and creates the
# `rustproxy` service user with its working and config directories.
#
# The sample configs are copied to /etc/rustproxy/ as a starting point; edit
# them to taste before starting the services.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
UNIT_DIR="${UNIT_DIR:-/etc/systemd/system}"
SERVICE_USER="${SERVICE_USER:-rustproxy}"
CONFIG_DIR="${CONFIG_DIR:-/etc/rustproxy}"
DATA_DIR="${DATA_DIR:-/var/lib/rustproxy}"

install -d -m 0755 "$CONFIG_DIR" "$DATA_DIR" "$UNIT_DIR"

# Provision the unprivileged service user if absent.
if ! id "$SERVICE_USER" >/dev/null 2>&1; then
  echo "==> creating service user '$SERVICE_USER'"
  useradd --system --home-dir "$DATA_DIR" --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi
chown "$SERVICE_USER":"$SERVICE_USER" "$DATA_DIR"

# Copy the deployed units (gateway + control-plane + node template).
echo "==> installing units from deploy/systemd/"
install -m 0644 "$REPO/deploy/systemd/"rustproxy-*.service "$UNIT_DIR/"

# Seed sample configs (only if not already present) as a starting point.
for svc in control-plane gateway node; do
  if [ ! -f "$CONFIG_DIR/$svc.toml" ]; then
    echo "==> seeding $CONFIG_DIR/$svc.toml"
    install -m 0644 "$REPO/configs/$svc.toml" "$CONFIG_DIR/$svc.toml"
  fi
done

# The control-plane writes its SQLite DB into its working directory.
mkdir -p "$DATA_DIR"
chown "$SERVICE_USER":"$SERVICE_USER" "$DATA_DIR"

systemctl daemon-reload
echo
echo "==> done. Start services with, e.g.:"
echo "    systemctl enable --now rustproxy-control-plane"
echo "    systemctl enable --now rustproxy-gateway"
echo "    systemctl enable --now rustproxy-node@node-1"
echo
echo "    NOTE: edit $CONFIG_DIR/*.toml (sqlite_path, addrs, node-id) as needed."
