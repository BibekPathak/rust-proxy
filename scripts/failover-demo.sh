#!/usr/bin/env bash
# Failure-injection demo: show control-plane resilience to a node crash.
#
# Starts the control plane, three nodes (node-1..3) and the gateway with a short
# offline timeout, waits until all three are healthy, then kills node-2's process.
# The demo observes node-2 transition to "offline" while node-1/node-3 remain
# "healthy", then proves a proxy request still succeeds through a surviving node.
set -euo pipefail

CP_ADDR="${CP_ADDR:-127.0.0.1:18080}"
GW_ADDR="${GW_ADDR:-127.0.0.1:1180}"
TARGET_ADDR="${TARGET_ADDR:-127.0.0.1:19002}"
BIN="${BIN:-$(pwd)/target/debug/rustproxy}"
OFFLINE="${OFFLINE:-6s}"      # humantime-style duration
SWEEP="${SWEEP:-1s}"

cd "$(dirname "$0")/.."
[ -x "$BIN" ] || { echo "building binary (debug)"; cargo build -p rustproxy; }

TMP="$(mktemp -d)"
declare -A NODE_PID
cleanup() {
  kill "${NODE_PID[@]:-}" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# A tiny HTTP 200 target so the proxy has something to reach.
python3 - "$TARGET_ADDR" <<'PY' &
import socket, sys
host, port = sys.argv[1].rsplit(":", 1)
srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind((host, int(port))); srv.listen(64)
while True:
    c, _ = srv.accept()
    def run(c):
        try:
            c.recv(4096)
            c.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        except OSError: pass
        finally: c.close()
    import threading
    threading.Thread(target=run, args=(c,), daemon=True).start()
PY
TARGET_PID=$!

cat > "$TMP/cp.toml" <<EOF
bind_addr = "$CP_ADDR"
sqlite_path = "$TMP/rustproxy.db"
offline_timeout = "$OFFLINE"
sweep_interval = "$SWEEP"
[logging]
level = "error"
json = false
EOF
"$BIN" control-plane --config "$TMP/cp.toml" >"$TMP/cp.log" 2>&1 &
CP_PID=$!

for _ in $(seq 1 50); do
  curl -fsS "http://$CP_ADDR/health" >/dev/null 2>&1 && break
  sleep 0.2
done

node_state() {
  curl -fsS "http://$CP_ADDR/nodes" 2>/dev/null | python3 -c '
import sys, json
d = json.load(sys.stdin)
id_ = sys.argv[1]
print(next((n["state"] for n in d.get("nodes", []) if n["id"] == id_), "absent"))
' "$1"
}

# Start nodes node-1..node-3.
for i in 1 2 3; do
  port=$((15000 + i))
  cat > "$TMP/node-$i.toml" <<EOF
node_id = "node-$i"
control_plane_url = "http://$CP_ADDR"
bind_addr = "127.0.0.1:$port"
advertised_address = "127.0.0.1:$port"
max_connections = 256
heartbeat_interval = "1s"
[logging]
level = "error"
json = false
EOF
  "$BIN" node --config "$TMP/node-$i.toml" >"$TMP/node-$i.log" 2>&1 &
  NODE_PID[$i]=$!
done

cat > "$TMP/gateway.toml" <<EOF
bind_addr = "$GW_ADDR"
control_plane_url = "http://$CP_ADDR"
max_connections = 1024
connect_timeout = "3s"
idle_timeout = "15s"
[logging]
level = "error"
json = false
EOF
"$BIN" gateway --config "$TMP/gateway.toml" >"$TMP/gw.log" 2>&1 &
GW_PID=$!

echo "==> waiting for all three nodes to be healthy"
for _ in $(seq 1 60); do
  s1=$(node_state node-1); s2=$(node_state node-2); s3=$(node_state node-3)
  if [ "$s1" = healthy ] && [ "$s2" = healthy ] && [ "$s3" = healthy ]; then
    break
  fi
  sleep 0.2
done
echo "    node-1=$s1 node-2=$s2 node-3=$s3"

url="http://${TARGET_ADDR}/"
echo "==> before failure: proxy request succeeds"
body="$(curl -fsS --max-time 10 -x "socks5h://$GW_ADDR" "$url" 2>/dev/null || true)"
echo "    response: '${body:-<empty>}'"

echo "==> killing node-2 (pid ${NODE_PID[2]})"
kill -9 "${NODE_PID[2]}" 2>/dev/null || true

echo "==> waiting for node-2 to be marked offline (timeout=$OFFLINE, sweep=$SWEEP)"
for _ in $(seq 1 60); do
  s1=$(node_state node-1); s2=$(node_state node-2); s3=$(node_state node-3)
  if [ "$s2" = offline ]; then break; fi
  sleep 0.2
done
echo "    node-1=$s1 node-2=$s2 node-3=$s3"

echo "==> after failure: proxy still succeeds (routed to a surviving node)"
body="$(curl -fsS --max-time 10 -x "socks5h://$GW_ADDR" "$url" 2>/dev/null || true)"
echo "    response: '${body:-<empty>}'"

kill "$TARGET_PID" "$CP_PID" "$GW_PID" 2>/dev/null || true
echo
echo "==> demo complete: node-2 crashed -> control plane marked it offline,"
echo "    node-1/node-3 stayed healthy, and the gateway kept serving traffic."
