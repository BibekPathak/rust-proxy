//! Per-service configuration models.
//!
//! All runtime parameters are configurable via TOML files (or environment
//! overrides where noted). Nothing service-critical is hard-coded; each
//! service binary reads its own configuration through a `FromStr`/serde
//! deserialization from a `--config` path.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::logging::LogLevel;

/// Logging-related options shared by every service.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LoggingConfig {
    /// Global log level.
    #[serde(default)]
    pub level: LogLevel,
    /// Emit logs as JSON lines (useful under systemd/containers).
    #[serde(default)]
    pub json: bool,
}

/// Control plane service configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlPlaneConfig {
    /// Address the Axum HTTP API binds to.
    pub bind_addr: SocketAddr,
    /// SQLite database path (registry persistence).
    pub sqlite_path: PathBuf,
    /// A node is marked offline if it has not heartbeated within this window.
    #[serde(with = "humantime_serde", default = "default_offline_timeout")]
    pub offline_timeout: Duration,
    /// Refresh interval of the background offline-detection task.
    #[serde(with = "humantime_serde", default = "default_sweep_interval")]
    pub sweep_interval: Duration,
    /// Optional bearer token required for mutating control plane endpoints.
    #[serde(default)]
    pub api_token: Option<String>,
    /// Logging options.
    #[serde(default)]
    pub logging: LoggingConfig,
}

fn default_offline_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_sweep_interval() -> Duration {
    Duration::from_secs(5)
}

/// Gateway service configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    /// Address the SOCKS5 gateway binds to.
    pub bind_addr: SocketAddr,
    /// Base URL of the control plane API, e.g. `http://127.0.0.1:8080`.
    pub control_plane_url: String,
    /// Optional bearer token forwarded to the control plane for auth.
    #[serde(default)]
    pub api_token: Option<String>,
    /// If set, require SOCKS5 username/password authentication with these
    /// credentials; otherwise the gateway offers no-authentication only.
    #[serde(default)]
    pub socks_username: Option<String>,
    /// Password paired with `socks_username`.
    #[serde(default)]
    pub socks_password: Option<String>,
    /// Maximum number of simultaneous client connections (backpressure).
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Connection establishment timeout when connecting to a selected node.
    #[serde(with = "humantime_serde", default = "default_connect_timeout")]
    pub connect_timeout: Duration,
    /// Idle timeout after which a proxied connection is torn down.
    #[serde(with = "humantime_serde", default = "default_idle_timeout")]
    pub idle_timeout: Duration,
    /// Optional address for an HTTP `/metrics` endpoint served by the gateway.
    #[serde(default)]
    pub metrics_bind_addr: Option<SocketAddr>,
    /// Logging options.
    #[serde(default)]
    pub logging: LoggingConfig,
}

fn default_max_connections() -> usize {
    1024
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_idle_timeout() -> Duration {
    Duration::from_secs(300)
}

/// Proxy node agent configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeConfig {
    /// Unique identifier for this node.
    pub node_id: String,
    /// Base URL of the control plane API.
    pub control_plane_url: String,
    /// Optional bearer token used to authenticate with the control plane.
    #[serde(default)]
    pub api_token: Option<String>,
    /// Address the node listens on for gateway forwarding requests.
    pub bind_addr: SocketAddr,
    /// Address this node advertises to the control plane for gateway
    /// forwarding. This lets a node bind to `0.0.0.0` internally while
    /// advertising a container/service name reachable by the gateway
    /// (e.g. `node-1:20001`). Defaults to `bind_addr` when unset.
    #[serde(default)]
    pub advertised_address: Option<String>,
    /// Overall bandwidth limit in bytes/second this node may serve.
    #[serde(default = "default_bandwidth_limit")]
    pub bandwidth_limit: u64,
    /// Maximum number of simultaneous proxied connections.
    #[serde(default = "default_node_max_connections")]
    pub max_connections: usize,
    /// Interval between heartbeats sent to the control plane.
    #[serde(with = "humantime_serde", default = "default_heartbeat_interval")]
    pub heartbeat_interval: Duration,
    /// Timeout for establishing the upstream connection to a target.
    #[serde(with = "humantime_serde", default = "default_upstream_timeout")]
    pub upstream_timeout: Duration,
    /// Logging options.
    #[serde(default)]
    pub logging: LoggingConfig,
}

fn default_bandwidth_limit() -> u64 {
    10 * 1024 * 1024 // 10 MiB/s
}

fn default_node_max_connections() -> usize {
    128
}

fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(5)
}

fn default_upstream_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Load a config of type `T` from a TOML file path.
pub fn load<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> crate::Result<T> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Config(format!("failed to read {}: {e}", path.display())))?;
    let cfg = toml::from_str(&raw)
        .map_err(|e| crate::Error::Config(format!("failed to parse {}: {e}", path.display())))?;
    Ok(cfg)
}
