//! Optional HTTP `/metrics` endpoint served by the gateway.
//!
//! If `metrics_bind_addr` is configured in the TOML, the gateway starts a
//! tiny Axum listener alongside the SOCKS5 acceptor that exposes Prometheus
//! metrics.

use axum::{routing::get, Router};

async fn metrics_endpoint() -> impl axum::response::IntoResponse {
    let body = crate::metrics::render();
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

async fn health_endpoint() -> impl axum::response::IntoResponse {
    axum::Json(serde_json::json!({"status":"ok"}))
}

fn metrics_router() -> Router {
    Router::new()
        .route("/metrics", get(metrics_endpoint))
        .route("/health", get(health_endpoint))
}

/// Run the Prometheus metrics server on `addr` until `shutdown` is signalled.
pub async fn run(addr: std::net::SocketAddr, shutdown: tokio_util::sync::CancellationToken) {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "failed to bind metrics listener");
            return;
        }
    };
    tracing::info!(%addr, "gateway metrics listening");
    let app = metrics_router();
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await;
    tracing::info!("gateway metrics listener shut down");
}
