//! Integration tests for the SQLite node registry.
//!
//! These run against a real in-memory SQLite database to exercise the SQL
//! queries and migrations, not a mock.

use chrono::Utc;
use rustproxy_control_plane::model::NodeState;
use rustproxy_control_plane::registry::{
    migrate, Heartbeat, NodeRepository, RegisterRequest, SqliteNodeRepository,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

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

async fn repo() -> (SqliteNodeRepository, SqlitePool) {
    let pool = pool().await;
    migrate(&pool).await.unwrap();
    (SqliteNodeRepository::new(pool.clone()), pool)
}

fn register(id: &str) -> RegisterRequest {
    RegisterRequest {
        node_id: id.to_string(),
        address: format!("127.0.0.1:20{id}"),
        max_connections: 100,
        bandwidth_limit: 1_000_000,
    }
}

#[tokio::test]
async fn migrate_applies_schema() {
    let pool = pool().await;
    migrate(&pool).await.unwrap();
    // The nodes table must exist after migration.
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='nodes'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, 1);
}

#[tokio::test]
async fn register_get_round_trip() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();

    let node = repo.get("a").await.unwrap().expect("node exists");
    assert_eq!(node.id, "a");
    assert_eq!(node.address, "127.0.0.1:20a");
    assert_eq!(node.state, NodeState::Starting);
    assert_eq!(node.max_connections, 100);
    assert_eq!(node.active_connections, 0);
    assert!(node.last_heartbeat.is_none());
}

#[tokio::test]
async fn register_is_idempotent_and_listing() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();
    // Re-registering the same id must not duplicate it.
    repo.register(&register("a")).await.unwrap();
    repo.register(&register("b")).await.unwrap();

    let nodes = repo.list().await.unwrap();
    assert_eq!(nodes.len(), 2);
    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&"a") && ids.contains(&"b"));
}

#[tokio::test]
async fn heartbeat_updates_metrics() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();

    repo.apply_heartbeat(&Heartbeat {
        node_id: "a".to_string(),
        active_connections: 7,
        bytes_up: 1000,
        bytes_down: 2000,
        latency_ms: 42,
        available_bandwidth: 500_000,
    })
    .await
    .unwrap();

    let node = repo.get("a").await.unwrap().unwrap();
    assert_eq!(node.active_connections, 7);
    assert_eq!(node.bytes_up, 1000);
    assert_eq!(node.bytes_down, 2000);
    assert_eq!(node.latency_ms, 42);
    assert!(node.last_heartbeat.is_some());
    let age = Utc::now().signed_duration_since(node.last_heartbeat.unwrap());
    assert!(age.num_seconds() < 30);
}

#[tokio::test]
async fn state_transitions_persist() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();

    repo.set_draining("a").await.unwrap();
    assert_eq!(
        repo.get("a").await.unwrap().unwrap().state,
        NodeState::Draining
    );

    repo.mark_offline("a").await.unwrap();
    assert_eq!(
        repo.get("a").await.unwrap().unwrap().state,
        NodeState::Offline
    );
}

#[tokio::test]
async fn active_connection_adjustment_is_bounded() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();

    repo.adjust_active("a", 3).await.unwrap();
    repo.adjust_active("a", -1).await.unwrap();
    assert_eq!(repo.get("a").await.unwrap().unwrap().active_connections, 2);

    // Negative deltas must not push the count below zero.
    repo.adjust_active("a", -100).await.unwrap();
    assert_eq!(repo.get("a").await.unwrap().unwrap().active_connections, 0);
}

#[tokio::test]
async fn byte_accounting_accumulates() {
    let (repo, _pool) = repo().await;
    repo.register(&register("a")).await.unwrap();

    repo.accumulate_bytes("a", 500, 250).await.unwrap();
    repo.accumulate_bytes("a", 100, 75).await.unwrap();

    let node = repo.get("a").await.unwrap().unwrap();
    assert_eq!(node.bytes_up, 600);
    assert_eq!(node.bytes_down, 325);
}

#[tokio::test]
async fn get_missing_returns_none() {
    let (repo, _pool) = repo().await;
    assert!(repo.get("nope").await.unwrap().is_none());
}
