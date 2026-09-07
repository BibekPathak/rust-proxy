//! Node health classification and state transitions.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::model::NodeState;

/// A configurable policy that governs how node health is evaluated.
#[derive(Debug, Clone, Copy)]
pub struct HealthPolicy {
    /// How long a node may be silent before it is considered offline.
    pub offline_timeout: Duration,
    /// Utilization ratio above which a healthy node is reclassified degraded.
    pub degraded_threshold: f64,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            offline_timeout: Duration::from_secs(30),
            degraded_threshold: 0.9,
        }
    }
}

/// Recompute the health classification for a live node that just heartbeated.
///
/// Preserves `Draining` and `Offline` (a draining node stays draining
/// regardless of load); otherwise classifies as `Healthy` or `Degraded`
/// based on connection utilization relative to [`HealthPolicy`].
pub fn reclassify_after_heartbeat(
    current: NodeState,
    active_connections: u32,
    max_connections: u32,
    policy: &HealthPolicy,
) -> NodeState {
    match current {
        NodeState::Draining | NodeState::Offline => current,
        _ => {
            let utilization = if max_connections == 0 {
                1.0
            } else {
                active_connections as f64 / max_connections as f64
            };
            if utilization >= policy.degraded_threshold {
                NodeState::Degraded
            } else {
                NodeState::Healthy
            }
        }
    }
}

/// Whether a node should be marked offline because its heartbeat is stale.
pub fn is_stale(
    last_heartbeat: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    policy: &HealthPolicy,
) -> bool {
    match last_heartbeat {
        None => false, // node is the initiator of staleness only after a first beat
        Some(ts) => {
            let elapsed = now
                .signed_duration_since(ts)
                .to_std()
                .unwrap_or(Duration::ZERO);
            elapsed >= policy.offline_timeout
        }
    }
}

/// The target state when a draining node reaches zero active connections.
pub fn drained_complete_state() -> NodeState {
    NodeState::Offline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> HealthPolicy {
        HealthPolicy {
            offline_timeout: Duration::from_secs(30),
            degraded_threshold: 0.9,
        }
    }

    #[test]
    fn healthy_when_below_threshold() {
        let s = reclassify_after_heartbeat(NodeState::Starting, 4, 100, &policy());
        assert_eq!(s, NodeState::Healthy);
    }

    #[test]
    fn degraded_when_at_threshold() {
        let s = reclassify_after_heartbeat(NodeState::Healthy, 90, 100, &policy());
        assert_eq!(s, NodeState::Degraded);
    }

    #[test]
    fn draining_is_preserved() {
        let s = reclassify_after_heartbeat(NodeState::Draining, 0, 100, &policy());
        assert_eq!(s, NodeState::Draining);
    }

    #[test]
    fn stale_detection() {
        let now = Utc::now();
        let old = Some(now - chrono::Duration::seconds(31));
        assert!(is_stale(old, now, &policy()));
        let recent = Some(now - chrono::Duration::seconds(5));
        assert!(!is_stale(recent, now, &policy()));
        assert!(!is_stale(None, now, &policy()));
    }
}
