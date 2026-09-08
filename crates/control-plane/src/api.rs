//! Axum HTTP API for the control plane.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Node, NodeState};
use crate::registry::{Heartbeat, RegisterRequest};
use crate::service::SharedService;

/// Application state shared across handlers.
#[derive(Clone)]
pub struct AppState {
    pub service: SharedService,
    pub auth: Auth,
}

/// Authentication policy for the API.
#[derive(Debug, Clone)]
pub enum Auth {
    /// No authentication required.
    None,
    /// A bearer token must be presented on all requests.
    Bearer(String),
}

/// Application error mapped to an HTTP response.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: self.message,
        });
        (self.status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// A node as exposed to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResponse {
    pub id: String,
    pub address: String,
    pub state: NodeState,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub max_connections: u32,
    pub active_connections: u32,
    pub bandwidth_limit: u64,
    pub available_bandwidth: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub latency_ms: u32,
}

impl NodeResponse {
    fn from_node(n: &Node, available_bandwidth: u64) -> Self {
        Self {
            id: n.id.clone(),
            address: n.address.clone(),
            state: n.state,
            last_heartbeat: n.last_heartbeat,
            max_connections: n.max_connections,
            active_connections: n.active_connections,
            bandwidth_limit: n.bandwidth_limit,
            available_bandwidth,
            bytes_up: n.bytes_up,
            bytes_down: n.bytes_down,
            latency_ms: n.latency_ms,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NodeListResponse {
    pub nodes: Vec<NodeResponse>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_register(req: &RegisterRequest) -> Result<(), ApiError> {
    if req.node_id.trim().is_empty() {
        return Err(ApiError::bad_request("node_id must not be empty"));
    }
    if req.address.trim().is_empty() {
        return Err(ApiError::bad_request("address must not be empty"));
    }
    if req.max_connections == 0 {
        return Err(ApiError::bad_request("max_connections must be > 0"));
    }
    if req.bandwidth_limit == 0 {
        return Err(ApiError::bad_request("bandwidth_limit must be > 0"));
    }
    Ok(())
}

fn validate_heartbeat(hb: &Heartbeat) -> Result<(), ApiError> {
    if hb.node_id.trim().is_empty() {
        return Err(ApiError::bad_request("node_id must not be empty"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

async fn authorize(headers: &HeaderMap, auth: &Auth) -> Result<(), ApiError> {
    match auth {
        Auth::None => Ok(()),
        Auth::Bearer(expected) => {
            let provided = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
            if provided == expected.as_str() {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid bearer token",
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn health(State(state): State<AppState>) -> ApiResult<HealthResponse> {
    // Liveness check; also verifies the DB is reachable.
    state
        .service
        .list()
        .await
        .map_err(|_| ApiError::internal("storage unavailable"))?;
    Ok(Json(HealthResponse { status: "ok" }))
}

pub async fn list_nodes(State(state): State<AppState>) -> ApiResult<NodeListResponse> {
    let nodes = state
        .service
        .list()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut out = Vec::with_capacity(nodes.len());
    for n in &nodes {
        out.push(NodeResponse::from_node(n, n.bandwidth_limit));
    }
    Ok(Json(NodeListResponse { nodes: out }))
}

pub async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<NodeResponse> {
    let node = state
        .service
        .get(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("node '{id}' not found")))?;
    Ok(Json(NodeResponse::from_node(&node, node.bandwidth_limit)))
}

/// Select the best eligible node to route a new connection through.
///
/// Returns `503 Service Unavailable` when no healthy node is available.
pub async fn select_node(State(state): State<AppState>) -> ApiResult<NodeResponse> {
    let node = state
        .service
        .select()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| {
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no healthy node available")
        })?;
    Ok(Json(NodeResponse::from_node(&node, node.bandwidth_limit)))
}

pub async fn register_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<NodeResponse> {
    authorize(&headers, &state.auth).await?;
    validate_register(&req)?;
    let node = state
        .service
        .register(&req)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(NodeResponse::from_node(&node, node.bandwidth_limit)))
}

pub async fn heartbeat_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut hb): Json<Heartbeat>,
) -> ApiResult<HealthResponse> {
    authorize(&headers, &state.auth).await?;
    hb.node_id = id;
    validate_heartbeat(&hb)?;
    state
        .service
        .heartbeat(&hb)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(HealthResponse { status: "ok" }))
}

pub async fn drain_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<NodeResponse> {
    authorize(&headers, &state.auth).await?;
    let node = state
        .service
        .drain(&id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("node '{id}' not found")))?;
    Ok(Json(NodeResponse::from_node(&node, node.bandwidth_limit)))
}

pub async fn metrics() -> impl IntoResponse {
    let body = crate::metrics::render();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

/// Build the Axum router for the control plane API.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/nodes", get(list_nodes))
        .route("/nodes/select", post(select_node))
        .route("/nodes/:id", get(get_node))
        .route("/nodes/register", post(register_node))
        .route("/nodes/:id/heartbeat", post(heartbeat_node))
        .route("/nodes/:id/drain", post(drain_node))
        .route("/metrics", get(metrics))
        .with_state(state)
}
