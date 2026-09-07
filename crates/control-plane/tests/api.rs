//! Integration tests for the control plane HTTP API.
//!
//! These spin up the real Axum router over an in-memory SQLite repository and
//! issue requests with `tower::ServiceExt::oneshot`.

use std::str::FromStr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use rustproxy_control_plane::api::AppState;
use rustproxy_control_plane::health::HealthPolicy;
use rustproxy_control_plane::registry::{migrate, SqliteNodeRepository};
use rustproxy_control_plane::service::ControlPlaneService;
use rustproxy_control_plane::{router, Auth};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tower::ServiceExt;

async fn pool() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap()
}

async fn app_state_with_token(token: Option<&str>) -> AppState {
    let pool = pool().await;
    migrate(&pool).await.unwrap();
    let service = ControlPlaneService::from_repo_sql(
        SqliteNodeRepository::new(pool),
        HealthPolicy::default(),
    );
    let auth = match token {
        Some(t) => Auth::Bearer(t.to_string()),
        None => Auth::None,
    };
    AppState {
        service: Arc::new(service),
        auth,
    }
}

async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = match body {
        Some(b) => builder.body(Body::from(b.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn register_body(id: &str) -> String {
    format!(
        r#"{{"node_id":"{id}","address":"127.0.0.1:20{id}","max_connections":100,"bandwidth_limit":1000000}}"#
    )
}

#[tokio::test]
async fn health_endpoint_ok() {
    let state = app_state_with_token(None).await;
    let app = router(state);
    let (status, body) = send(app, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ok"));
}

#[tokio::test]
async fn register_then_list_and_get() {
    let state = app_state_with_token(None).await;
    let app = router(state.clone());

    let (status, _) = send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(app.clone(), "GET", "/nodes", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("n1"));

    let (status, body) = send(app, "GET", "/nodes/n1", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"state\":\"starting\""));
    assert!(body.contains("\"active_connections\":0"));
}

#[tokio::test]
async fn get_unknown_node_is_404() {
    let state = app_state_with_token(None).await;
    let app = router(state);
    let (status, _) = send(app, "GET", "/nodes/ghost", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn heartbeat_updates_node_and_state() {
    let state = app_state_with_token(None).await;
    let app = router(state.clone());
    send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        None,
    )
    .await;

    let hb = r#"{"node_id":"n1","active_connections":3,"bytes_up":100,"bytes_down":200,"latency_ms":5,"available_bandwidth":900000}"#;
    let (status, _) = send(app.clone(), "POST", "/nodes/n1/heartbeat", Some(hb), None).await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(app, "GET", "/nodes/n1", None, None).await;
    assert!(body.contains("\"active_connections\":3"));
    assert!(body.contains("\"bytes_up\":100"));
    assert!(body.contains("\"latency_ms\":5"));
    assert!(body.contains("\"state\":\"healthy\""));
}

#[tokio::test]
async fn drain_marks_node_draining() {
    let state = app_state_with_token(None).await;
    let app = router(state.clone());
    send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        None,
    )
    .await;

    let (status, body) = send(app, "POST", "/nodes/n1/drain", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"state\":\"draining\""));
}

#[tokio::test]
async fn registration_is_validated() {
    let state = app_state_with_token(None).await;
    let app = router(state);
    // Empty node id.
    let bad = r#"{"node_id":"","address":"x","max_connections":10,"bandwidth_limit":100}"#;
    let (status, _) = send(app.clone(), "POST", "/nodes/register", Some(bad), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Zero max connections.
    let bad2 = r#"{"node_id":"x","address":"x","max_connections":0,"bandwidth_limit":100}"#;
    let (status, _) = send(app, "POST", "/nodes/register", Some(bad2), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn auth_required_when_configured() {
    let state = app_state_with_token(Some("secret")).await;
    let app = router(state.clone());

    // Without a token -> 401.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // With the wrong token -> 401.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        Some("wrong"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // With the correct token -> 200.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        Some("secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn metrics_endpoint_renders_prometheus() {
    metrics_install_guard();
    let state = app_state_with_token(None).await;
    let app = router(state.clone());
    // Record a connection and a node so the metrics have content.
    rustproxy_control_plane::metrics::record_connection_opened();
    send(
        app.clone(),
        "POST",
        "/nodes/register",
        Some(&register_body("n1")),
        None,
    )
    .await;

    let (status, body) = send(app, "GET", "/metrics", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("proxy_connections_total"));
    assert!(body.contains("proxy_active_connections"));
    assert!(body.contains("proxy_node_health"));
}

/// The metrics recorder installs once globally; tests may run concurrently, so
/// call init() defensively via a shared once.
fn metrics_install_guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        rustproxy_control_plane::metrics::init();
    });
}
