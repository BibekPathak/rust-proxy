//! Gateway metrics (Prometheus-compatible).
//!
//! Records connection-level proxy metrics into the global [`metrics`] registry.
//! The gateway binary can expose them at a `/metrics` HTTP endpoint via
//! [`metrics::render`].

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// The installed render handle, initialised exactly once.
static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the gateway metrics recorder and declare metric metadata.
///
/// Safe to call repeatedly; only the first call installs the recorder.
pub fn init() -> &'static PrometheusHandle {
    HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("gateway metrics recorder installed");

        metrics::describe_counter!("proxy_connections_total", "Total proxy connections opened.");
        metrics::describe_gauge!(
            "proxy_active_connections",
            "Currently active proxy connections."
        );
        metrics::describe_counter!(
            "proxy_connection_errors_total",
            "Proxy connection failures."
        );
        metrics::describe_counter!("proxy_bytes_up_total", "Bytes upstream (client -> target).");
        metrics::describe_counter!(
            "proxy_bytes_down_total",
            "Bytes downstream (target -> client)."
        );

        handle
    })
}

/// Prometheus text exposition of gateway metrics.
pub fn render() -> String {
    init().render()
}

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
