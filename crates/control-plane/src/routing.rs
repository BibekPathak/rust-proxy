//! Health-aware node selection.
//!
//! The selector is deliberately deterministic (no randomness) so behaviour is
//! testable. Each eligible node is scored and the highest score wins; ties
//! resolve to the first eligible node in list order.

use crate::model::{Node, NodeState};

/// Relative importance of each factor in the composite score.
#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub health: f64,
    pub capacity: f64,
    pub latency: f64,
    pub bandwidth: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            health: 100.0,
            capacity: 1.0,
            latency: 1.0,
            bandwidth: 1.0,
        }
    }
}

/// Strategy for picking the best node to route a connection through.
pub trait NodeSelector: Send + Sync {
    /// Return the node to use, or `None` if no node is currently eligible.
    fn select(&self, nodes: &[Node]) -> Option<Node>;
}

/// A health- and capacity-aware selector driven by a weighted composite score.
///
/// Score(health × capacity × latency × bandwidth) is evaluated per eligible
/// node; the node with the maximum score is returned.
#[derive(Debug, Clone, Copy, Default)]
pub struct HealthAwareSelector {
    weights: ScoreWeights,
}

impl HealthAwareSelector {
    pub fn new(weights: ScoreWeights) -> Self {
        Self { weights }
    }
}

/// The health factor in `[0, 1]`: healthy nodes score full marks, degraded
/// nodes are penalised, and non-eligible nodes are excluded entirely.
fn health_factor(state: NodeState) -> f64 {
    match state {
        NodeState::Healthy => 1.0,
        NodeState::Degraded => 0.5,
        _ => 0.0,
    }
}

/// The capacity factor in `[0, 1]`: fraction of connection slots remaining.
fn capacity_factor(node: &Node) -> f64 {
    if node.max_connections == 0 {
        return 0.0;
    }
    node.available_capacity() as f64 / node.max_connections as f64
}

/// The latency factor in `(0, 1]`: lower latency scores higher.
fn latency_factor(latency_ms: u32) -> f64 {
    1.0 / (1.0 + latency_ms as f64)
}

/// The bandwidth factor in `(0, 1]`: fraction of bandwidth not yet consumed.
fn bandwidth_factor(node: &Node) -> f64 {
    1.0 - node.bandwidth_utilization()
}

/// Compute the weighted composite score for a single node.
///
/// Only eligible (selectable) nodes produce a positive score; all others
/// evaluate to `0.0` and are ignored by the selector.
pub fn score(node: &Node, weights: &ScoreWeights) -> f64 {
    if !node.is_selectable() {
        return 0.0;
    }
    let health = health_factor(node.state);
    let capacity = capacity_factor(node);
    let latency = latency_factor(node.latency_ms);
    let bandwidth = bandwidth_factor(node);

    weights.health
        * health
        * (1.0 + weights.capacity * capacity)
        * (1.0 + weights.latency * latency)
        * (1.0 + weights.bandwidth * bandwidth)
}

impl NodeSelector for HealthAwareSelector {
    fn select(&self, nodes: &[Node]) -> Option<Node> {
        nodes
            .iter()
            .filter(|n| n.is_selectable())
            .max_by(|a, b| {
                score(a, &self.weights)
                    .partial_cmp(&score(b, &self.weights))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn node(id: &str, state: NodeState, active: u32, max: u32, latency: u32) -> Node {
        Node {
            id: id.to_string(),
            address: format!("127.0.0.1:{id}"),
            state,
            last_heartbeat: Some(Utc::now()),
            max_connections: max,
            active_connections: active,
            bandwidth_limit: 1_000_000,
            bytes_up: 0,
            bytes_down: 0,
            latency_ms: latency,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn unhealthy_nodes_are_excluded() {
        let nodes = [
            node("a", NodeState::Healthy, 1, 10, 10),
            node("b", NodeState::Draining, 0, 10, 5),
            node("c", NodeState::Offline, 0, 10, 5),
        ];
        let found = HealthAwareSelector::default().select(&nodes).unwrap();
        assert_eq!(found.id, "a");
    }

    #[test]
    fn returns_none_when_no_eligible_node() {
        let nodes = [
            node("a", NodeState::Draining, 0, 10, 5),
            node("b", NodeState::Offline, 0, 10, 5),
        ];
        assert!(HealthAwareSelector::default().select(&nodes).is_none());
    }

    #[test]
    fn prefers_lower_latency_between_equal_capacity() {
        let nodes = [
            node("slow", NodeState::Healthy, 1, 10, 100),
            node("fast", NodeState::Healthy, 1, 10, 5),
        ];
        let found = HealthAwareSelector::default().select(&nodes).unwrap();
        assert_eq!(found.id, "fast");
    }

    #[test]
    fn prefers_more_available_capacity() {
        let nodes = [
            node("full", NodeState::Healthy, 9, 10, 10),
            node("roomy", NodeState::Healthy, 1, 10, 10),
        ];
        let found = HealthAwareSelector::default().select(&nodes).unwrap();
        assert_eq!(found.id, "roomy");
    }

    #[test]
    fn avoids_overloaded_nodes() {
        let nodes = [
            node("loaded", NodeState::Healthy, 10, 10, 5),
            node("deg", NodeState::Degraded, 5, 10, 5),
            node("clear", NodeState::Healthy, 2, 10, 10),
        ];
        let found = HealthAwareSelector::default().select(&nodes).unwrap();
        assert_eq!(found.id, "clear");
    }

    #[test]
    fn degraded_loses_to_healthy_at_similar_load() {
        let nodes = [
            node("deg", NodeState::Degraded, 2, 10, 5),
            node("healthy", NodeState::Healthy, 2, 10, 5),
        ];
        let found = HealthAwareSelector::default().select(&nodes).unwrap();
        assert_eq!(found.id, "healthy");
    }

    #[test]
    fn score_is_deterministic_across_runs() {
        let nodes = [node("a", NodeState::Healthy, 3, 10, 20)];
        let w = ScoreWeights::default();
        assert_eq!(score(&nodes[0], &w), score(&nodes[0], &w));
    }
}
