#!/usr/bin/env bash
# SOCKS5 load benchmark.
#
# Stands up the full RustProxy stack (control plane, one node, gateway) plus a
# local high-throughput discard target, then drives it with the `loadgen`
# example across a sweep of concurrency levels. Printed throughput and success
# figures come from the loadgen itself (measured at runtime).
set -euo pipefail

# Config (ports/values are tuned for a typical dev box; override as desired).
GW_ADDR="${GW_ADDR:-127.0.0.1:1180}"
TARGET_ADDR="${TARGET_ADDR:-127.0.0.1:19001}"
CP_ADDR="${CP_ADDR:-127.0.0.1:18080}"
NODE_ADDR="${NODE_ADDR:-127.0.0.1:12001}"
DURATION="${DURATION:-6}"
CONNS="${CONNS:-64}"
SWEEP="${SWEEP:-1,8,16,32,64,128}"
BIN="${BIN:-$(pwd)/target/debug/rustproxy}"

cd "$(dirname "$0")/.."

# Build the debug binary and loadgen if not already present.
if [ ! -x "$BIN" ]; then
  echo "==> building binary (debug)"
  cargo build -p rustproxy
fi
echo "==> building loadgen"
cargo build -p rustproxy --example loadgen
LOADGEN="$(pwd)/target/debug/examples/loadgen"

TMP="$(mktemp -d)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$TMP"
}
trap cleanup EXIT

# A tiny discard target (reads and drops bytes so sockets never back up).
python3 - "$TARGET_ADDR" <<'PY' &
import socket, sys
host, port = sys.argv[1].rsplit(":", 1)
srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind((host, int(port))); srv.listen(512)
while True:
    c, _ = srv.accept()
    def run(c):
        try:
            while True:
                if not c.recv(1 << 16): break
        except OSError: pass
        finally: c.close()
    import threading
    threading.Thread(target=run, args=(c,), daemon=True).start()
PY
PIDS+=($!)
sleep 0.3

# Control plane config.
cat > "$TMP/cp.toml" <<EOF
bind_addr = "$CP_ADDR"
sqlite_path = "$TMP/rustproxy.db"
offline_timeout = "10s"
sweep_interval = "2s"
[logging]
level = "error"
json = false
EOF
"$BIN" control-plane --config "$TMP/cp.toml" >"$TMP/cp.log" 2>&1 &
PIDS+=($!)

# Wait for the control plane to be ready.
for _ in $(seq 1 50); do
  if curl -fsS "http://$CP_ADDR/health" >/dev/null 2>&1; then break; fi
  sleep 0.2
done

# Node config (advertises a locally reachable address for the gateway).
cat > "$TMP/node.toml" <<EOF
node_id = "node-bench"
control_plane_url = "http://$CP_ADDR"
bind_addr = "$NODE_ADDR"
advertised_address = "$NODE_ADDR"
max_connections = 4096
heartbeat_interval = "1s"
[logging]
level = "error"
json = false
EOF
"$BIN" node --config "$TMP/node.toml" >"$TMP/node.log" 2>&1 &
PIDS+=($!)

# Gateway config.
cat > "$TMP/gateway.toml" <<EOF
bind_addr = "$GW_ADDR"
control_plane_url = "http://$CP_ADDR"
max_connections = 8192
connect_timeout = "5s"
idle_timeout = "30s"
[logging]
level = "error"
json = false
EOF
"$BIN" gateway --config "$TMP/gateway.toml" >"$TMP/gw.log" 2>&1 &
PIDS+=($!)

# Wait for a healthy node.
for _ in $(seq 1 60); do
  if curl -fsS "http://$CP_ADDR/nodes" 2>/dev/null | grep -q '"healthy"'; then
    break
  fi
  sleep 0.2
done

echo "==> benchmark: target=$TARGET_ADDR gateway=$GW_ADDR duration=${DURATION}s"
echo "Concurrency ->  Conn_OK  Conn_Fail  MiB/s  Success%"

IFS=',' read -ra LEVELS <<<"$SWEEP"
headline=""
for n in "${LEVELS[@]}"; do
  out="$("$LOADGEN" --gateway "$GW_ADDR" --target "$TARGET_ADDR" --conns "$n" --duration "$DURATION" 2>/dev/null || true)"
  # out looks like: connections=.. ok=.. failed=.. elapsed=.. bytes=.. throughput=.. MiB/s success=..
  ok="$(echo "$out" | sed -n 's/.*ok=\([0-9]*\).*/\1/p')"
  fail="$(echo "$out" | sed -n 's/.*failed=\([0-9]*\).*/\1/p')"
  mib="$(echo "$out" | sed -n 's/.*throughput=\([0-9.]*\).*/\1/p')"
  succ="$(echo "$out" | sed -n 's/.*success=\([0-9.]*\)%.*/\1/p')"
  [ -z "$ok" ] && ok=0; [ -z "$fail" ] && fail=0
  [ -z "$mib" ] && mib=0; [ -z "$succ" ] && succ=0
  printf '      %-4s  ->     %-5s     %-5s  %-6s  %s%%\n' "$n" "$ok" "$fail" "$mib" "$succ"
  if [ "$n" = "$CONNS" ]; then
    headline="Sustained ${n} concurrent SOCKS5 connections at ~${mib} MiB/s with ${succ}% success"
  fi
done

echo
echo "==> headline"
echo "$headline"
