# RustProxy

A distributed SOCKS5 routing and node-orchestration system written in Rust.

RustProxy is an asynchronous, multi-node proxy control plane. Clients connect to a
**gateway** that negotiates the SOCKS5 protocol, asks a central **control plane** which
**node** is best placed to carry the connection, and then chains a SOCKS5 `CONNECT`
through that node out to the real target. Each node reports health, load, and transfer
statistics to the control plane, which uses that signal to route and enforce capacity.

It is designed to be deployable as a small set of Linux services (bare metal, systemd, or
Docker Compose) and is observability-friendly out of the box (Prometheus metrics on every
service).

---

## Architecture

```text
                 control plane (HTTP API + SQLite registry)
                    ▲            ▲
        register /  │  heartbeat │ /nodes/select (best node)
        heartbeats   │            │
                    │            │
        +-----------+------------+-------------+
        |   gateway (SOCKS5)     |   node agent (SOCKS5)      |
        |   :1080                |   :20001                   |
        +-----------+------------+-------------+
            ▲              |                    |
        clients       chained CONNECT           │
                       ┌────────┴────────┐
                       ▼                 ▼
                SOCKS5 data          real target
```

A request flows like this:

1. A client opens a SOCKS5 connection to the **gateway** (`:1080`).
2. The gateway negotiates the SOCKS5 handshake (optionally RFC 1929 username/password).
3. The gateway reads the `CONNECT` request and asks the control plane
   `POST /nodes/select` for the best eligible node.
4. The gateway opens a second SOCKS5 session to that **node**, chaining the same
   `CONNECT` through it.
5. The node opens a TCP connection to the real destination, replies success, and the
   gateway and node splice the two byte streams so data flows bidirectionally.
6. Traffic and connection statistics accumulate and are reported to the control plane.

---

## Components

| Crate | Role |
|-------|------|
| `crates/common` | Shared config, structured logging, CLI definition, typed errors |
| `crates/protocol` | Pure, I/O-free SOCKS5 (RFC 1928/1929) parsing and serialization |
| `crates/control-plane` | Node registry, health-aware routing, bandwidth allocation, Axum HTTP API, Prometheus metrics |
| `crates/gateway` | User-facing SOCKS5 listener that selects nodes and chains connections |
| `crates/node` | Per-node agent: registration, heartbeat loop, SOCKS5 forwarding, metrics |
| `crates/rustproxy-bin` | The single `rustproxy` binary with subcommands and graceful shutdown |

### Control plane API

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/health` | Liveness probe (also checks SQLite reachability) |
| `GET` | `/nodes` | List all registered nodes |
| `GET` | `/nodes/:id` | Fetch one node |
| `POST` | `/nodes/register` | Register a node |
| `POST` | `/nodes/:id/heartbeat` | Report health/load from a node |
| `POST` | `/nodes/:id/drain` | Mark a node draining |
| `POST` | `/nodes/select` | Select the best eligible node (returns `503` if none) |
| `GET` | `/metrics` | Prometheus text exposition |

Mutating endpoints can require a bearer token via the `api_token` config option.

### Metrics

Metrics are exposed with the Prometheus text format and are per-service:

- **Control plane** (`/metrics`): node gauges, routing/selection counts, bandwidth gauges.
- **Gateway** (optional `metrics_bind_addr`): `proxy_connections_total`,
  `proxy_active_connections`, `proxy_connection_errors_total`, `proxy_bytes_up_total`,
  `proxy_bytes_down_total`.
- **Node**: `node_connections_total`, `node_active_connections`,
  `node_connection_errors_total`, `node_bytes_up_total`, `node_bytes_down_total`,
  `node_heartbeats_total`, `node_heartbeat_errors_total`.

---

## Building

Requires Rust (the project is a Cargo workspace) and SQLx's `migrate` feature to embed and
apply the SQLite migrations.

```bash
cargo build --release
```

The resulting `rustproxy` binary is at `target/release/rustproxy`.

```text
Usage: rustproxy <COMMAND>

Commands:
  gateway        Run the SOCKS5 gateway that routes client traffic through nodes
  node           Run a proxy node agent
  control-plane  Run the control plane API server
  migrate        Apply SQLite migrations for a database
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### Quality gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

---

## Configuration

Each service is a TOML file passed with `--config <FILE>`. Sample files live in
[`configs/`](configs).

### Control plane (`configs/control-plane.toml`)

| Key | Description | Default |
|-----|-------------|---------|
| `bind_addr` | Axum HTTP bind address | required |
| `sqlite_path` | SQLite database path | required |
| `offline_timeout` | Mark a node offline if no heartbeat within this window | `30s` |
| `sweep_interval` | Background offline-detection scan interval | `5s` |
| `api_token` | Optional bearer token for mutating endpoints | none |

### Gateway (`configs/gateway.toml`)

| Key | Description | Default |
|-----|-------------|---------|
| `bind_addr` | SOCKS5 listener address | required |
| `control_plane_url` | Control plane base URL | required |
| `api_token` | Token forwarded to the control plane | none |
| `socks_username` / `socks_password` | Require SOCKS5 user/pass auth on the gateway | off |
| `max_connections` | Concurrent-connection backpressure ceiling | `1024` |
| `connect_timeout` | Node connect / handshake timeout | `10s` |
| `idle_timeout` | Tear down idle proxied connections | `300s` |
| `metrics_bind_addr` | Optional HTTP `/metrics` endpoint | none |

### Node (`configs/node.toml`)

| Key | Description | Default |
|-----|-------------|---------|
| `node_id` | Unique identifier | required |
| `control_plane_url` | Control plane base URL | required |
| `api_token` | Token used with the control plane | none |
| `bind_addr` | Listener for gateway forwarding | required |
| `advertised_address` | Address to publish to the control plane (e.g. a Docker service name) | `bind_addr` |
| `bandwidth_limit` | Bandwidth ceiling in bytes/second | `10 MiB/s` |
| `max_connections` | Concurrent-connection ceiling | `128` |
| `heartbeat_interval` | Interval between heartbeats | `5s` |
| `upstream_timeout` | Timeout connecting to a target | `10s` |

### Example — run locally

```bash
# 1) migrate & start the control plane
rustproxy migrate --config configs/control-plane.toml
rustproxy control-plane --config configs/control-plane.toml &

# 2) start a node
rustproxy node --config configs/node.toml &

# 3) start the gateway
rustproxy gateway --config configs/gateway.toml &

# 4) proxy a request through the whole chain
curl -x socks5h://127.0.0.1:1080 http://example.com/
```

---

## Docker

A single-node Docker Compose demo is provided in [`deploy/docker/`](deploy/docker). It runs
control-plane, one node, the gateway, and an `nginx` target container, then proxies a
request through the full chain.

```bash
docker compose -f deploy/docker/docker-compose.yml up --build
```

Verify the control-plane has registered the healthy node:

```bash
curl http://127.0.0.1:8080/nodes
```

Then proxy a request (resolving `target` inside the compose network):

```bash
curl -x socks5h://127.0.0.1:1080 http://target/
```

The `Dockerfile` is multi-stage (Rust builder → slim runtime with `ca-certificates` and
`curl`); containers run as a non-root user via an entrypoint that normalizes the data
volume. Default ports: control-plane `8080`, gateway `1080`, gateway metrics `19090`,
node `20001`.

---

## Layout

```text
configs/            Sample TOML configuration files
crates/             Workspace member crates
deploy/docker/      Dockerfile, entrypoint, configs, and compose demo
deploy/systemd/     Systemd unit files
migrations/         SQLite schema migrations (embedded by SQLx)
scripts/            Helper scripts
tests/              Workspace-level integration tests
```

---

## License

MIT
