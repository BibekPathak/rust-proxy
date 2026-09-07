//! Control plane: node registry, routing, bandwidth allocation, and Axum API.
//!
//! This module is the core (Phase 2) — it has no HTTP concerns yet; the Axum
//! API is layered on top of these services in a later phase.

pub mod bandwidth;
pub mod health;
pub mod model;
pub mod registry;
pub mod routing;

pub use bandwidth::{BandwidthAllocator, BandwidthGuard, RateLimiter};
pub use health::HealthPolicy;
pub use model::{Node, NodeState};
pub use registry::{migrate, Heartbeat, NodeRepository, RegisterRequest, SqliteNodeRepository};
pub use routing::{score, HealthAwareSelector, NodeSelector, ScoreWeights};
