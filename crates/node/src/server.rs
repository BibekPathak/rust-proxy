//! Node agent server: accept loop, heartbeat loop, and graceful shutdown.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::cp_client::{ControlPlaneClient, HeartbeatPayload, RegisterPayload};
use crate::handler::{handle_connection, NodeContext};
use crate::metrics;
use rustproxy_common::config::NodeConfig;

/// Shared state for tracking connection metrics across heartbeats.
struct NodeStats {
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
    active_connections: AtomicU32,
}

impl NodeStats {
    fn new() -> Self {
        Self {
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
            active_connections: AtomicU32::new(0),
        }
    }

    fn snapshot_and_reset(&self) -> (u64, u64, u32) {
        let up = self.bytes_up.swap(0, Ordering::Relaxed);
        let down = self.bytes_down.swap(0, Ordering::Relaxed);
        let active = self.active_connections.load(Ordering::Relaxed);
        (up, down, active)
    }
}

/// The node agent server.
pub struct NodeServer {
    config: NodeConfig,
    client: ControlPlaneClient,
}

impl NodeServer {
    pub fn new(config: NodeConfig) -> Self {
        let client = ControlPlaneClient::new(&config.control_plane_url, config.api_token.clone());
        Self { config, client }
    }

    /// Run the node agent until `shutdown` is signalled.
    pub async fn run(self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let stats = Arc::new(NodeStats::new());

        // 1. Register with the control plane
        let public_address = self
            .config
            .advertised_address
            .clone()
            .unwrap_or_else(|| self.config.bind_addr.to_string());
        let reg = RegisterPayload {
            node_id: self.config.node_id.clone(),
            address: public_address,
            max_connections: self.config.max_connections as u32,
            bandwidth_limit: self.config.bandwidth_limit,
        };
        match self.client.register(&reg).await {
            Ok(()) => info!(node_id = %self.config.node_id, "registered with control plane"),
            Err(e) => {
                warn!(error = %e, "initial registration failed; will retry in heartbeat loop");
            }
        }

        // 2. Start heartbeat loop
        let heartbeat_handle = {
            let client = self.client.clone();
            let node_id = self.config.node_id.clone();
            let interval = self.config.heartbeat_interval;
            let shutdown = shutdown.clone();
            let stats = stats.clone();
            let bandwidth_limit = self.config.bandwidth_limit;
            tokio::spawn(async move {
                heartbeat_loop(client, &node_id, interval, bandwidth_limit, stats, shutdown).await;
            })
        };

        // 3. Start SOCKS5 accept loop
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        info!(%local_addr, "node agent listening");

        let ctx = NodeContext {
            auth: None,
            handshake_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
        };
        let sem = Arc::new(Semaphore::new(self.config.max_connections));
        let shutdown = shutdown.clone();

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer)) => {
                            debug!(%peer, "accepted gateway connection");
                            let ctx = ctx.clone();
                            let sem = Arc::clone(&sem);
                            let shutdown = shutdown.clone();
                            let stats = stats.clone();
                            tokio::spawn(async move {
                                let _permit = match sem.acquire().await {
                                    Ok(p) => p,
                                    Err(_) => {
                                        warn!(%peer, "node overloaded; rejecting");
                                        return;
                                    }
                                };
                                if shutdown.is_cancelled() {
                                    debug!("shutting down; dropping connection");
                                    return;
                                }
                                stats.active_connections.fetch_add(1, Ordering::Relaxed);
                                handle_connection(stream, ctx).await;
                                stats.active_connections.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "accept failed");
                        }
                    }
                }
                _ = shutdown.cancelled() => {
                    info!("node shutdown signalled");
                    break;
                }
            }
        }

        // Drain: wait for in-flight connections with a deadline
        let deadline = Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            if stats.active_connections.load(Ordering::Relaxed) == 0 || start.elapsed() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        heartbeat_handle.abort();
        info!("node agent shut down cleanly");
        Ok(())
    }
}

/// Periodic heartbeat loop: sends stats to the control plane.
async fn heartbeat_loop(
    client: ControlPlaneClient,
    node_id: &str,
    interval: Duration,
    bandwidth_limit: u64,
    stats: Arc<NodeStats>,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(interval);
    tick.tick().await; // skip immediate first tick
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("heartbeat loop stopped");
                break;
            }
            _ = tick.tick() => {
                let (up, down, active) = stats.snapshot_and_reset();
                let hb = HeartbeatPayload {
                    active_connections: active,
                    bytes_up: up,
                    bytes_down: down,
                    latency_ms: 0,
                    available_bandwidth: bandwidth_limit,
                };
                match client.heartbeat(node_id, &hb).await {
                    Ok(()) => {
                        metrics::record_heartbeat();
                        debug!(node_id, active, up, down, "heartbeat sent");
                    }
                    Err(e) => {
                        metrics::record_heartbeat_error();
                        warn!(error = %e, "heartbeat failed");
                    }
                }
            }
        }
    }
}
