#!/usr/bin/env bash
# Build (if needed) and run control-plane, a node, and the gateway locally
# using the sample configs. Ctrl-C (or any job exit) cleans up all services.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN="${BIN:-$(pwd)/target/debug/rustproxy}"

# Build a debug binary if the configured one is missing.
if [ ! -x "$BIN" ]; then
  echo "==> binary not found at $BIN; building (debug)"
  cargo build -p rustproxy
  BIN="$(pwd)/target/debug/rustproxy"
fi

PIDS=()
cleanup() {
  echo
  echo "==> stopping rustproxy services"
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT

echo "==> applying migrations"
"$BIN" migrate --config configs/control-plane.toml

echo "==> starting control plane"
"$BIN" control-plane --config configs/control-plane.toml &
PIDS+=($!)

echo "==> starting node"
"$BIN" node --config configs/node.toml &
PIDS+=($!)

echo "==> starting gateway"
"$BIN" gateway --config configs/gateway.toml &
PIDS+=($!)

echo "==> all services started; Ctrl-C to stop"
wait
