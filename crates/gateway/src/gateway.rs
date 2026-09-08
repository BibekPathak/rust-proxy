//! SOCKS5 gateway server: accept loop, backpressure, and graceful shutdown.

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::handler::{handle_connection, GatewayContext};
use crate::proxy_client::ControlPlaneClient;
use rustproxy_common::config::GatewayConfig;

/// The gateway server: accepts SOCKS5 clients and dispatches them through
/// handler tasks, enforcing a global connection ceiling.
pub struct Gateway {
    ctx: GatewayContext,
    sem: Arc<Semaphore>,
    config: GatewayConfig,
}

impl Gateway {
    pub fn new(config: GatewayConfig, client: ControlPlaneClient) -> Self {
        let auth = match (&config.socks_username, &config.socks_password) {
            (Some(user), Some(pass)) => Some((user.clone(), pass.clone())),
            _ => None,
        };
        let ctx = GatewayContext {
            client,
            auth,
            connect_timeout: config.connect_timeout,
            idle_timeout: config.idle_timeout,
        };
        let sem = Arc::new(Semaphore::new(config.max_connections));
        Self { ctx, sem, config }
    }

    /// Run the gateway until `shutdown` is signalled.
    pub async fn run(self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        info!(%local_addr, "socks5 gateway listening");

        // Spawn metrics server if configured.
        let mut metrics_handle = None;
        if let Some(addr) = self.config.metrics_bind_addr {
            let sd = shutdown.clone();
            metrics_handle = Some(tokio::spawn(async move {
                crate::metrics_server::run(addr, sd).await;
            }));
        }

        let shutdown = shutdown.clone();
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer)) => {
                            debug!(%peer, "accepted client");
                            let ctx = self.ctx.clone();
                            let sem = Arc::clone(&self.sem);
                            let shutdown = shutdown.clone();
                            tokio::spawn(async move {
                                let _permit = match sem.acquire().await {
                                    Ok(p) => p,
                                    Err(_) => {
                                        warn!(%peer, "gateway overloaded; rejecting");
                                        return;
                                    }
                                };
                                // If the gateway is shutting down, stop accepting new work.
                                if shutdown.is_cancelled() {
                                    debug!("shutting down; dropping client");
                                    return;
                                }
                                handle_connection(stream, ctx).await;
                                // _permit dropped here, freeing the semaphore slot
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "accept failed");
                        }
                    }
                }
                _ = shutdown.cancelled() => {
                    info!("gateway shutdown signalled");
                    break;
                }
            }
        }

        // Wait for in-flight tasks to drain (best-effort with a deadline).
        let deadline = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            if Arc::strong_count(&self.sem) <= 1 || start.elapsed() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        if let Some(h) = metrics_handle {
            h.abort();
        }
        info!("gateway shut down cleanly");
        Ok(())
    }
}
