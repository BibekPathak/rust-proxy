//! Prometheus-compatible metrics.
//!
//! Metrics are emitted through the [`metrics`] crate with a single global
//! recorder installed once. The exporter is built with `metrics-exporter-prometheus`
//! and exposed at `/metrics` via [`render`].
//!
//! Labeled gauges (per node) use `node_id` as the label so Prometheus can
//! aggregate across the fleet.
//!
//! Note on the `metrics` macro API: `counter!`/`gauge!` return handles (e.g.
//! [`metrics::Counter`], [`metrics::Gauge`]); values are applied via
//! `.increment()`/`.set()`. The Prometheus exporter retains registered
//! metrics across handle drops, so one-shot registration is safe.

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// The installed render handle, initialised exactly once.
static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

const NODE: &str = "node_id";

/// Install the global metrics recorder and declare metric metadata.
///
/// Safe to call multiple times; only the first call has any effect.
pub fn init() -> &'static PrometheusHandle {
    HANDLE.get_or_init(|| {
        let builder = PrometheusBuilder::new();
        let handle = builder
            .install_recorder()
            .expect("metrics recorder installed");

        metrics::describe_counter!(
            "proxy_connections_total",
            "Total number of proxy connections handled (opened)."
        );
        metrics::describe_gauge!(
            "proxy_active_connections",
            "Currently active proxy connections."
        );
        metrics::describe_counter!(
            "proxy_connection_errors_total",
            "Total number of proxy connection failures."
        );
        metrics::describe_counter!("proxy_bytes_up_total", "Bytes upstream (client -> target).");
        metrics::describe_counter!(
            "proxy_bytes_down_total",
            "Bytes downstream (target -> client)."
        );
        metrics::describe_gauge!(
            "proxy_node_health",
            "Per-node health: 1 healthy, 0.5 degraded, 0 otherwise."
        );
        metrics::describe_gauge!("proxy_node_latency_ms", "Per-node reported latency in ms.");
        metrics::describe_gauge!(
            "proxy_bandwidth_utilization",
            "Per-node bandwidth utilization in [0,1]."
        );

        handle
    })
}

/// Build a Prometheus text-format exposition of the recorded metrics.
pub fn render() -> String {
    init().render()
}

// -- connection lifecycle -----------------------------------------------------

pub fn record_connection_opened() {
    metrics::counter!("proxy_connections_total").increment(1);
    metrics::gauge!("proxy_active_connections").increment(1.0);
}

pub fn record_connection_closed() {
    metrics::gauge!("proxy_active_connections").decrement(1.0);
}

pub fn record_connection_error() {
    metrics::counter!("proxy_connection_errors_total").increment(1);
}

pub fn record_bytes_up(n: u64) {
    metrics::counter!("proxy_bytes_up_total").increment(n);
}

pub fn record_bytes_down(n: u64) {
    metrics::counter!("proxy_bytes_down_total").increment(n);
}

// -- per-node state -----------------------------------------------------------

/// Health value for a node used in `proxy_node_health`.
pub fn health_gauge_value(state: crate::model::NodeState) -> f64 {
    match state {
        crate::model::NodeState::Healthy => 1.0,
        crate::model::NodeState::Degraded => 0.5,
        _ => 0.0,
    }
}

pub fn record_node(node: &crate::model::Node) {
    metrics::gauge!("proxy_node_health", NODE => node.id.clone())
        .set(health_gauge_value(node.state));
    metrics::gauge!("proxy_node_latency_ms", NODE => node.id.clone()).set(node.latency_ms as f64);
    metrics::gauge!("proxy_bandwidth_utilization", NODE => node.id.clone())
        .set(node.bandwidth_utilization());
}
