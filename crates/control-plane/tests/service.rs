//! Integration tests for the control plane service layer.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rustproxy_control_plane::health::HealthPolicy;
use rustproxy_control_plane::model::NodeState;
use rustproxy_control_plane::registry::{
    migrate, Heartbeat, RegisterRequest, SqliteNodeRepository,
};
use rustproxy_control_plane::service::ControlPlaneService;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

async fn service() -> (Arc<ControlPlaneService>, SqlitePool) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    migrate(&pool).await.unwrap();
    let policy = HealthPolicy {
        offline_timeout: Duration::from_secs(60),
        degraded_threshold: 0.9,
    };
    let s = Arc::new(ControlPlaneService::from_repo_sql(
        SqliteNodeRepository::new(pool.clone()),
        policy,
    ));
    (s, pool)
}

fn register(id: &str) -> RegisterRequest {
    RegisterRequest {
        node_id: id.to_string(),
        address: format!("127.0.0.1:20{id}"),
        max_connections: 100,
        bandwidth_limit: 1_000_000,
    }
}

fn hb(id: &str, active: u32) -> Heartbeat {
    Heartbeat {
        node_id: id.to_string(),
        active_connections: active,
        bytes_up: 0,
        bytes_down: 0,
        latency_ms: 10,
        available_bandwidth: 500_000,
    }
}

#[tokio::test]
async fn select_skips_draining_and_unregistered_nodes() {
    let (s, _pool) = service().await;
    // a: drains once requested; c: never heartbeats (stays Starting).
    s.register(&register("a")).await.unwrap();
    s.register(&register("b")).await.unwrap();
    s.register(&register("c")).await.unwrap();
    s.heartbeat(&hb("a", 1)).await.unwrap();
    s.heartbeat(&hb("b", 1)).await.unwrap();
    s.drain("a").await.unwrap();

    let selected = s.select().await.unwrap().unwrap();
    assert_eq!(selected.id, "b");
}

#[tokio::test]
async fn heartbeats_reclassify_as_degraded_under_load() {
    let (s, _pool) = service().await;
    s.register(&register("a")).await.unwrap();
    s.heartbeat(&hb("a", 95)).await.unwrap();
    let node = s.get("a").await.unwrap().unwrap();
    assert_eq!(node.state, NodeState::Degraded);
}

#[tokio::test]
async fn fresh_node_is_healthy_after_heartbeat() {
    let (s, _pool) = service().await;
    s.register(&register("a")).await.unwrap();
    s.heartbeat(&hb("a", 1)).await.unwrap();
    let node = s.get("a").await.unwrap().unwrap();
    assert_eq!(node.state, NodeState::Healthy);
}

#[tokio::test]
async fn stale_nodes_are_marked_offline() {
    let (s, pool) = service().await;
    s.register(&register("stale")).await.unwrap();
    s.heartbeat(&hb("stale", 1)).await.unwrap();

    // Artificially age the heartbeat far into the past.
    let age_secs = 10_000;
    let now = Utc::now().timestamp() - age_secs;
    sqlx::query("UPDATE nodes SET last_heartbeat = ? WHERE id = ?")
        .bind(now)
        .bind("stale")
        .execute(&pool)
        .await
        .unwrap();

    let marked = s.mark_stale_offline().await.unwrap();
    assert_eq!(marked, 1);
    assert_eq!(
        s.get("stale").await.unwrap().unwrap().state,
        NodeState::Offline
    );
}

#[tokio::test]
async fn fresh_nodes_are_not_marked_offline() {
    let (s, _pool) = service().await;
    s.register(&register("fresh")).await.unwrap();
    s.heartbeat(&hb("fresh", 1)).await.unwrap();
    let marked = s.mark_stale_offline().await.unwrap();
    assert_eq!(marked, 0);
    assert_eq!(
        s.get("fresh").await.unwrap().unwrap().state,
        NodeState::Healthy
    );
}

#[tokio::test]
async fn select_with_bandwidth_guards_capacity() {
    let (s, _pool) = service().await;
    // 4 slots share a 4000-byte budget -> 1000 bytes/slot; 4 reservations fill it.
    let mut req = register("thin");
    req.max_connections = 4;
    req.bandwidth_limit = 4000;
    s.register(&req).await.unwrap();
    s.heartbeat(&hb("thin", 0)).await.unwrap();

    let mut guards = Vec::new();
    for _ in 0..4 {
        guards.push(s.select_with_bandwidth().await.unwrap().unwrap().guard);
    }
    // All slots are bandwidth-reserved; a further selection must be refused.
    assert!(s.select_with_bandwidth().await.unwrap().is_none());
    drop(guards);
    // Releasing frees capacity again.
    assert!(s.select_with_bandwidth().await.unwrap().is_some());
}
