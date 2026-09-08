//! Tests for configuration deserialization and defaults.

use rustproxy_common::config::{self, GatewayConfig, LoggingConfig, NodeConfig};

#[test]
fn logging_default_is_info() {
    let cfg = LoggingConfig::default();
    assert_eq!(cfg.level, rustproxy_common::logging::LogLevel::Info);
    assert!(!cfg.json);
}

#[test]
fn gateway_config_uses_supplied_values_and_defaults() {
    let raw = r#"
        bind_addr = "127.0.0.1:1080"
        control_plane_url = "http://127.0.0.1:8080"
    "#;
    let cfg: GatewayConfig = toml::from_str(raw).unwrap();
    assert_eq!(cfg.bind_addr.to_string(), "127.0.0.1:1080");
    assert_eq!(cfg.control_plane_url, "http://127.0.0.1:8080");
    // Defaults applied:
    assert_eq!(cfg.max_connections, 1024);
    assert_eq!(cfg.logging.level, rustproxy_common::logging::LogLevel::Info);
}

#[test]
fn node_config_uses_supplied_values_and_defaults() {
    let raw = r#"
        node_id = "node-a"
        control_plane_url = "http://127.0.0.1:8080"
        bind_addr = "127.0.0.1:20001"
        bandwidth_limit = 5242880
    "#;
    let cfg: NodeConfig = toml::from_str(raw).unwrap();
    assert_eq!(cfg.node_id, "node-a");
    assert_eq!(cfg.bandwidth_limit, 5_242_880);
    // Defaults applied:
    assert_eq!(cfg.max_connections, 128);
    assert_eq!(cfg.heartbeat_interval, std::time::Duration::from_secs(5));
    // advertised_address defaults to None when unset.
    assert_eq!(cfg.advertised_address, None);
}

#[test]
fn node_config_parses_advertised_address() {
    let raw = r#"
        node_id = "node-a"
        control_plane_url = "http://127.0.0.1:8080"
        bind_addr = "0.0.0.0:20001"
        advertised_address = "node-a:20001"
    "#;
    let cfg: NodeConfig = toml::from_str(raw).unwrap();
    assert_eq!(cfg.bind_addr.to_string(), "0.0.0.0:20001");
    assert_eq!(cfg.advertised_address.as_deref(), Some("node-a:20001"));
}

#[test]
fn bundled_sample_configs_parse() {
    let base = env!("CARGO_MANIFEST_DIR");
    let cfg_root = format!("{base}/../..");
    let _gw: GatewayConfig = config::load(&std::path::PathBuf::from(format!(
        "{cfg_root}/configs/gateway.toml"
    )))
    .unwrap();
    let _cp: rustproxy_common::config::ControlPlaneConfig = config::load(
        &std::path::PathBuf::from(format!("{cfg_root}/configs/control-plane.toml")),
    )
    .unwrap();
}
