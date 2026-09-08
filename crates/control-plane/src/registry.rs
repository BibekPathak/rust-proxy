//! Node registry: persistence behind a clean abstraction.
//!
//! Routing and health logic depend only on the [`NodeRepository`] trait and
//! the plain [`Node`] value, never on SQL. The concrete
//! [`SqliteNodeRepository`] persists to SQLite via SQLx.

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::SqlitePool;

use crate::model::{Node, NodeState};

/// A heartbeat reported by a node agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    /// The node sending the heartbeat. The control plane resolves the id from
    /// the URL path, so it need not be present in the request body.
    #[serde(default)]
    pub node_id: String,
    pub active_connections: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub latency_ms: u32,
    pub available_bandwidth: u64,
}

/// Register a node with the control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub node_id: String,
    pub address: String,
    pub max_connections: u32,
    pub bandwidth_limit: u64,
}

/// Persistence interface for the node registry.
///
/// Implementations must be `Send + Sync` and performant under concurrent use.
#[async_trait]
pub trait NodeRepository: Send + Sync {
    /// Insert a new node (idempotent: an existing registration is updated).
    async fn register(&self, req: &RegisterRequest) -> anyhow::Result<()>;

    /// Fetch a single node by id.
    async fn get(&self, id: &str) -> anyhow::Result<Option<Node>>;

    /// List all nodes.
    async fn list(&self) -> anyhow::Result<Vec<Node>>;

    /// Apply a heartbeat, updating health metrics and `last_heartbeat`.
    async fn apply_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()>;

    /// Mark a node as draining.
    async fn set_draining(&self, id: &str) -> anyhow::Result<()>;

    /// Force a node into the `Offline` state (used by the sweep task).
    async fn mark_offline(&self, id: &str) -> anyhow::Result<()>;

    /// Set an arbitrary lifecycle state (used for health reclassification).
    async fn set_state(&self, id: &str, state: NodeState) -> anyhow::Result<()>;

    /// Atomically adjust a node's active-connection count by `delta`.
    async fn adjust_active(&self, id: &str, delta: i64) -> anyhow::Result<()>;

    /// Atomically accumulate transfer accounting for a node.
    async fn accumulate_bytes(&self, id: &str, up: u64, down: u64) -> anyhow::Result<()>;
}

const SELECT_COLS: &str = "id, address, status, last_heartbeat, max_connections, \
     active_connections, bandwidth_limit, bytes_up, bytes_down, latency_ms, created_at, updated_at";

fn row_into_node(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Node> {
    let state_str: String = row.try_get("status")?;
    let state = NodeState::parse(&state_str)
        .ok_or_else(|| anyhow!("unknown node state stored: {state_str}"))?;

    Ok(Node {
        id: row.try_get("id")?,
        address: row.try_get("address")?,
        state,
        last_heartbeat: opt_epoch(row.try_get("last_heartbeat")?),
        max_connections: row.try_get::<i64, _>("max_connections")? as u32,
        active_connections: row.try_get::<i64, _>("active_connections")? as u32,
        bandwidth_limit: row.try_get::<i64, _>("bandwidth_limit")? as u64,
        bytes_up: row.try_get::<i64, _>("bytes_up")? as u64,
        bytes_down: row.try_get::<i64, _>("bytes_down")? as u64,
        latency_ms: row.try_get::<i64, _>("latency_ms")? as u32,
        created_at: epoch(row.try_get::<i64, _>("created_at")?)?,
        updated_at: epoch(row.try_get::<i64, _>("updated_at")?)?,
    })
}

fn epoch(unix: i64) -> anyhow::Result<DateTime<Utc>> {
    DateTime::from_timestamp(unix, 0).ok_or_else(|| anyhow!("invalid epoch timestamp {unix}"))
}

fn opt_epoch(unix: Option<i64>) -> Option<DateTime<Utc>> {
    unix.and_then(|u| DateTime::from_timestamp(u, 0))
}

fn now_epoch() -> i64 {
    Utc::now().timestamp()
}

/// A [`NodeRepository`] backed by SQLite.
pub struct SqliteNodeRepository {
    pool: SqlitePool,
}

impl SqliteNodeRepository {
    /// Open a repository with migrations applied to `pool`.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for callers that run the server.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Apply pending migrations. Embedded from `../../migrations` at build time.
pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

#[async_trait]
impl NodeRepository for SqliteNodeRepository {
    async fn register(&self, req: &RegisterRequest) -> anyhow::Result<()> {
        let now = now_epoch();
        sqlx::query(
            "INSERT INTO nodes
                (id, address, status, last_heartbeat, max_connections, active_connections,
                 bandwidth_limit, bytes_up, bytes_down, latency_ms, created_at, updated_at)
             VALUES (?, ?, 'starting', NULL, ?, 0, ?, 0, 0, 0, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                address = excluded.address,
                max_connections = excluded.max_connections,
                bandwidth_limit = excluded.bandwidth_limit,
                status = CASE WHEN nodes.status IN ('offline','starting')
                              THEN 'starting' ELSE nodes.status END,
                updated_at = excluded.updated_at",
        )
        .bind(&req.node_id)
        .bind(&req.address)
        .bind(req.max_connections as i64)
        .bind(req.bandwidth_limit as i64)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Node>> {
        let row = sqlx::query(&format!("SELECT {SELECT_COLS} FROM nodes WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_into_node).transpose()
    }

    async fn list(&self) -> anyhow::Result<Vec<Node>> {
        let rows = sqlx::query(&format!("SELECT {SELECT_COLS} FROM nodes ORDER BY id"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_into_node).collect()
    }

    async fn apply_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        let now = now_epoch();
        sqlx::query(
            "UPDATE nodes SET
                last_heartbeat = ?,
                active_connections = ?,
                bytes_up = ?,
                bytes_down = ?,
                latency_ms = ?,
                updated_at = ?
             WHERE id = ?",
        )
        .bind(now)
        .bind(hb.active_connections as i64)
        .bind(hb.bytes_up as i64)
        .bind(hb.bytes_down as i64)
        .bind(hb.latency_ms as i64)
        .bind(now)
        .bind(&hb.node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_draining(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET status = 'draining', updated_at = ? WHERE id = ?")
            .bind(now_epoch())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn mark_offline(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET status = 'offline', updated_at = ? WHERE id = ?")
            .bind(now_epoch())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_state(&self, id: &str, state: NodeState) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET status = ?, updated_at = ? WHERE id = ?")
            .bind(state.as_str())
            .bind(now_epoch())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn adjust_active(&self, id: &str, delta: i64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE nodes SET active_connections = MAX(0, active_connections + ?),
                updated_at = ? WHERE id = ?",
        )
        .bind(delta)
        .bind(now_epoch())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn accumulate_bytes(&self, id: &str, up: u64, down: u64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE nodes SET bytes_up = bytes_up + ?, bytes_down = bytes_down + ?,
                updated_at = ? WHERE id = ?",
        )
        .bind(up as i64)
        .bind(down as i64)
        .bind(now_epoch())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
