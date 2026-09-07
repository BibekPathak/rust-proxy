//! Server wiring: the background health-sweep task and the HTTP listener.

use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::api::{router, AppState, Auth};
use crate::service::ControlPlaneService;

/// Spawn the background task that marks stale nodes offline.
///
/// Returns a handle that can be awaited to observe completion.
pub fn spawn_health_sweep(
    service: std::sync::Arc<ControlPlaneService>,
    interval: Duration,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        // Skip the immediate first tick so the database is fully ready.
        tick.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("health sweep stopped");
                    break;
                }
                _ = tick.tick() => {
                    match service.mark_stale_offline().await {
                        Ok(marked) => {
                            if marked > 0 {
                                info!(marked, "marked stale nodes offline");
                            }
                        }
                        Err(e) => warn!(error = %e, "health sweep failed"),
                    }
                }
            }
        }
    })
}

/// Serve the control plane API on the configured listener.
///
/// The server binds, starts the health-sweep task, and shuts down gracefully
/// (waiting for in-flight requests) when `shutdown` is cancelled.
pub async fn run(
    state: AppState,
    listener: TcpListener,
    sweep_interval: Duration,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let sweep = spawn_health_sweep(state.service.clone(), sweep_interval, shutdown.clone());

    let app = router(state);
    let addr = listener.local_addr()?;
    info!(%addr, "control plane listening");

    axum::serve(listener, app)
        .with_graceful_shutdown({
            let shutdown = shutdown.clone();
            async move { shutdown.cancelled().await }
        })
        .await?;

    let _ = sweep.await;
    info!("control plane shut down cleanly");
    Ok(())
}

/// Build an [`AppState`] from a service and optional bearer token.
pub fn app_state(
    service: std::sync::Arc<ControlPlaneService>,
    api_token: Option<String>,
) -> AppState {
    let auth = match api_token {
        Some(token) if !token.is_empty() => Auth::Bearer(token),
        _ => Auth::None,
    };
    AppState { service, auth }
}
