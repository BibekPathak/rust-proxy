#!/usr/bin/env bash
# End-to-end smoke test: verify the control plane is up and the full SOCKS5
# proxy chain (client -> gateway -> node -> target) works.
#
# Prereqs: control-plane, a node, and the gateway already running locally
# (e.g. via scripts/run-local.sh), with a reachable target. By default the
# target is https://example.com/.
set -euo pipefail

CP_URL="${CP_URL:-http://127.0.0.1:8080}"
GW_ADDR="${GW_ADDR:-127.0.0.1:1080}"
TARGET="${TARGET:-https://example.com/}"

echo "==> checking control plane health ($CP_URL/health)"
curl -fsS "$CP_URL/health" >/dev/null
echo "    ok"

echo "==> waiting for at least one healthy node"
for i in $(seq 1 20); do
  if curl -fsS "$CP_URL/nodes" | grep -q '"state":"healthy"'; then
    echo "    healthy node found after ${i}s"
    break
  fi
  if [ "$i" -eq 20 ]; then
    echo "    ERROR: no healthy node after 20s" >&2
    exit 1
  fi
  sleep 1
done

echo "==> proxying request through gateway ($GW_ADDR)"
body="$(curl -fsS --max-time 20 -x "socks5h://$GW_ADDR" "$TARGET")"
if [ -z "$body" ]; then
  echo "    ERROR: empty response through proxy" >&2
  exit 1
fi
echo "    received $(printf '%s' "$body" | wc -c) bytes"

echo "==> smoke test passed"
