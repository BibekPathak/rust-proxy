//! High-level control plane service.
//!
//! Coordinates the registry, the node selector, and per-node bandwidth
//! allocators. API handlers and other clients talk to this service, never to
//! SQL directly, keeping handlers thin per the project architecture.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::bandwidth::{BandwidthAllocator, BandwidthGuard};
use crate::health::{is_stale, reclassify_after_heartbeat, HealthPolicy};
use crate::model::{Node, NodeState};
use crate::registry::{Heartbeat, NodeRepository, RegisterRequest};
use crate::routing::NodeSelector;
use crate::routing::ScoreWeights;

/// A node selected to carry a new connection, together with a bandwidth
/// reservation that must be released when the connection ends.
#[derive(Debug)]
pub struct SelectedNode {
    pub node: Node,
    pub guard: BandwidthGuard,
}

/// Shared, thread-safe handle to the control plane service.
pub type SharedService = Arc<ControlPlaneService>;

/// The central service: registry + routing + bandwidth allocation.
pub struct ControlPlaneService {
    repo: Arc<dyn NodeRepository>,
    policy: HealthPolicy,
    weights: ScoreWeights,
    /// Per-node bandwidth allocators, created lazily from each node's limit.
    allocators: Mutex<HashMap<String, BandwidthAllocator>>,
}

impl ControlPlaneService {
    pub fn new(repo: Arc<dyn NodeRepository>, policy: HealthPolicy) -> Self {
        Self {
            repo,
            policy,
            weights: ScoreWeights::default(),
            allocators: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_repo_sql(
        repo: crate::registry::SqliteNodeRepository,
        policy: HealthPolicy,
    ) -> Self {
        Self::new(Arc::new(repo), policy)
    }

    /// The health policy governing this service.
    pub fn policy(&self) -> HealthPolicy {
        self.policy
    }

    fn allocator(&self, node_id: &str, limit: u64) -> BandwidthAllocator {
        let mut map = self.allocators.lock().unwrap();
        map.entry(node_id.to_string())
            .or_insert_with(|| BandwidthAllocator::new(limit))
            .clone()
    }

    /// Register a new node.
    pub async fn register(&self, req: &RegisterRequest) -> anyhow::Result<Node> {
        self.repo.register(req).await?;
        let node = self
            .repo
            .get(&req.node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("node disappeared after register"))?;
        crate::metrics::record_node(&node);
        Ok(node)
    }

    /// Apply a heartbeat and recompute the node's health state.
    pub async fn heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        let current = self.repo.get(&hb.node_id).await?;
        let (state, max) = match &current {
            Some(n) => (n.state, n.max_connections),
            None => return Ok(()), // unknown node; ignore silently
        };

        self.repo.apply_heartbeat(hb).await?;
        let next = reclassify_after_heartbeat(state, hb.active_connections, max, &self.policy);
        if next != state {
            self.repo.set_state(&hb.node_id, next).await?;
        }
        if let Some(n) = self.repo.get(&hb.node_id).await? {
            crate::metrics::record_node(&n);
        }
        Ok(())
    }

    /// List all registered nodes.
    pub async fn list(&self) -> anyhow::Result<Vec<Node>> {
        let nodes = self.repo.list().await?;
        for n in &nodes {
            crate::metrics::record_node(n);
        }
        Ok(nodes)
    }

    /// Fetch a single node.
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<Node>> {
        let n = self.repo.get(id).await?;
        if let Some(n) = &n {
            crate::metrics::record_node(n);
        }
        Ok(n)
    }

    /// Mark a node as draining and update its metric.
    pub async fn drain(&self, id: &str) -> anyhow::Result<Option<Node>> {
        self.repo.set_draining(id).await?;
        let n = self.repo.get(id).await?;
        if let Some(n) = &n {
            crate::metrics::record_node(n);
        }
        Ok(n)
    }

    /// Return the best eligible node by pure health-aware selection.
    pub async fn select(&self) -> anyhow::Result<Option<Node>> {
        let nodes = self.repo.list().await?;
        let eligible: Vec<Node> = nodes.into_iter().filter(|n| n.is_selectable()).collect();
        let selector = crate::routing::HealthAwareSelector::new(self.weights);
        Ok(selector.select(&eligible))
    }

    /// Select a node and reserve bandwidth on it, guarding against
    /// over-commitment. Candidates are tried in score order.
    pub async fn select_with_bandwidth(&self) -> anyhow::Result<Option<SelectedNode>> {
        let nodes = self.repo.list().await?;
        let mut eligible: Vec<Node> = nodes.into_iter().filter(|n| n.is_selectable()).collect();
        eligible.sort_by(|a, b| {
            crate::routing::score(b, &self.weights)
                .partial_cmp(&crate::routing::score(a, &self.weights))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for node in eligible {
            // Reserve an equal slice of the node's bandwidth per connection
            // slot; the sum of all reservations never exceeds the ceiling.
            let slots = node.max_connections.max(1) as u64;
            let share = (node.bandwidth_limit / slots).max(1);
            let alloc = self.allocator(&node.id, node.bandwidth_limit);
            if let Some(guard) = alloc.try_reserve(share) {
                return Ok(Some(SelectedNode { node, guard }));
            }
        }
        Ok(None)
    }

    /// Adjust a node's active connection count (e.g. on open/close).
    pub async fn adjust_active(&self, node_id: &str, delta: i64) -> anyhow::Result<()> {
        self.repo.adjust_active(node_id, delta).await
    }

    /// Mark any node whose heartbeat is stale as offline.
    ///
    /// Publishing nodes already `Offline` or `Draining` are untouched
    /// (a draining node transitions to offline only when its connections
    /// reach zero, handled by the node agent).
    pub async fn mark_stale_offline(&self) -> anyhow::Result<usize> {
        let now = chrono::Utc::now();
        let nodes = self.repo.list().await?;
        let mut marked = 0;
        for node in nodes {
            let live = matches!(
                node.state,
                NodeState::Starting | NodeState::Healthy | NodeState::Degraded
            );
            if live && is_stale(node.last_heartbeat, now, &self.policy) {
                self.repo.mark_offline(&node.id).await?;
                marked += 1;
            }
        }
        Ok(marked)
    }
}

impl std::fmt::Debug for ControlPlaneService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlPlaneService")
            .finish_non_exhaustive()
    }
}
