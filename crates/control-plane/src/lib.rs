//! Control plane: node registry, routing, bandwidth allocation, and Axum API.
//!
//! Phases:
//! * core (registry, routing, bandwidth, health) — no HTTP concerns
//! * API (Axum handlers, DTOs, auth) and Prometheus metrics layered on top

pub mod api;
pub mod bandwidth;
pub mod health;
pub mod metrics;
pub mod model;
pub mod registry;
pub mod routing;
pub mod server;
pub mod service;

pub use api::{router, AppState, Auth};
pub use bandwidth::{BandwidthAllocator, BandwidthGuard, RateLimiter};
pub use health::HealthPolicy;
pub use model::{Node, NodeState};
pub use registry::{migrate, Heartbeat, NodeRepository, RegisterRequest, SqliteNodeRepository};
pub use routing::{score, HealthAwareSelector, NodeSelector, ScoreWeights};
pub use server::run;
pub use service::ControlPlaneService;
