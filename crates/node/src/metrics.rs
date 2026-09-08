//! Node agent metrics (Prometheus-compatible).

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn init() -> &'static PrometheusHandle {
    HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("node metrics recorder installed");

        metrics::describe_counter!("node_connections_total", "Total connections handled.");
        metrics::describe_gauge!("node_active_connections", "Currently active connections.");
        metrics::describe_counter!("node_connection_errors_total", "Connection failures.");
        metrics::describe_counter!("node_bytes_up_total", "Bytes upstream (target -> client).");
        metrics::describe_counter!(
            "node_bytes_down_total",
            "Bytes downstream (client -> target)."
        );
        metrics::describe_counter!("node_heartbeats_total", "Total heartbeats sent.");
        metrics::describe_counter!("node_heartbeat_errors_total", "Failed heartbeat attempts.");

        handle
    })
}

pub fn render() -> String {
    init().render()
}

pub fn record_connection_opened() {
    metrics::counter!("node_connections_total").increment(1);
    metrics::gauge!("node_active_connections").increment(1.0);
}

pub fn record_connection_closed() {
    metrics::gauge!("node_active_connections").decrement(1.0);
}

pub fn record_connection_error() {
    metrics::counter!("node_connection_errors_total").increment(1);
}

pub fn record_bytes_up(n: u64) {
    metrics::counter!("node_bytes_up_total").increment(n);
}

pub fn record_bytes_down(n: u64) {
    metrics::counter!("node_bytes_down_total").increment(n);
}

pub fn record_heartbeat() {
    metrics::counter!("node_heartbeats_total").increment(1);
}

pub fn record_heartbeat_error() {
    metrics::counter!("node_heartbeat_errors_total").increment(1);
}
