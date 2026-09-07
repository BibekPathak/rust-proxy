//! Core domain model for proxy nodes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The lifecycle state of a node as seen by the control plane.
///
/// Transitions (enforced in [`crate::health`]):
///
/// ```text
///                 +----------+
///                 | Starting |
///                 +----------+
///                      |
///             first heartbeat
///                      v
///                 +---------+
///                 | Healthy |--(overloaded / degraded)--> Degraded
///                 +---------+                             |
///                      |                                 |
///                      +-----------(drain)---------------+
///                      v                                 v
///                 +---------+    (connections == 0)   +---------+
///                 | Draining|  ------------------->  | Offline |
///                 +---------+                        +---------+
///                      ^                                 ^
///                      +-----------(heartbeat lost) ------+
///                      +-----------(offline timeout) -----+
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Starting,
    Healthy,
    Degraded,
    Draining,
    Offline,
}

impl NodeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeState::Starting => "starting",
            NodeState::Healthy => "healthy",
            NodeState::Degraded => "degraded",
            NodeState::Draining => "draining",
            NodeState::Offline => "offline",
        }
    }

    pub fn parse(s: &str) -> Option<NodeState> {
        match s {
            "starting" => Some(NodeState::Starting),
            "healthy" => Some(NodeState::Healthy),
            "degraded" => Some(NodeState::Degraded),
            "draining" => Some(NodeState::Draining),
            "offline" => Some(NodeState::Offline),
            _ => None,
        }
    }
}

/// A proxy node registered with the control plane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    /// Stable unique identifier.
    pub id: String,
    /// The address of the node's forwarding listener, e.g. `127.0.0.1:20001`.
    pub address: String,
    /// Current lifecycle state.
    pub state: NodeState,
    /// Time of the most recent heartbeat, if any.
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// Maximum number of simultaneous proxied connections the node accepts.
    pub max_connections: u32,
    /// Number of currently active proxied connections.
    pub active_connections: u32,
    /// Configured bandwidth ceiling in bytes/second.
    pub bandwidth_limit: u64,
    /// Cumulative bytes transferred upstream (client -> target).
    pub bytes_up: u64,
    /// Cumulative bytes transferred downstream (target -> client).
    pub bytes_down: u64,
    /// Most recently reported round-trip latency in milliseconds.
    pub latency_ms: u32,
    /// When the registration was created.
    pub created_at: DateTime<Utc>,
    /// When the record was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Node {
    /// Whether the node can currently accept new work.
    ///
    /// Only nodes that are online and not draining are eligible. `Degraded`
    /// remains eligible but is penalised by the selector.
    pub fn is_selectable(&self) -> bool {
        matches!(self.state, NodeState::Healthy | NodeState::Degraded)
    }

    /// Remaining connection slots before hitting `max_connections`.
    pub fn available_capacity(&self) -> u32 {
        self.max_connections.saturating_sub(self.active_connections)
    }

    /// Bandwidth utilisation in `[0.0, 1.0]`, where `1.0` means saturated.
    ///
    /// We use the ratio of active connections to max connections as a proxy
    /// for instantaneous bandwidth pressure (exact byte-rate pressure is
    /// maintained separately by the allocator).
    pub fn bandwidth_utilization(&self) -> f64 {
        if self.max_connections == 0 {
            return 1.0;
        }
        let ratio = self.active_connections as f64 / self.max_connections as f64;
        ratio.clamp(0.0, 1.0)
    }
}
