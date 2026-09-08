//! Integration tests for the rustproxy binary.

use std::process::Command;

fn binary_path() -> String {
    let mut path = std::env::current_exe().unwrap();
    // The test binary is in target/debug/deps/, the real binary is in target/debug/.
    path.pop(); // remove test binary name
    path.pop(); // remove "deps"
    path.push("rustproxy");
    path.to_string_lossy().to_string()
}

#[test]
fn help_flag_exits_zero() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("failed to execute binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rustproxy"),
        "help should mention binary name"
    );
    assert!(
        stdout.contains("gateway"),
        "help should list gateway subcommand"
    );
    assert!(stdout.contains("node"), "help should list node subcommand");
    assert!(
        stdout.contains("control-plane"),
        "help should list control-plane subcommand"
    );
    assert!(
        stdout.contains("migrate"),
        "help should list migrate subcommand"
    );
}

#[test]
fn version_flag_exits_zero() {
    let output = Command::new(binary_path())
        .arg("--version")
        .output()
        .expect("failed to execute binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.1.0"),
        "version output should contain version number"
    );
}

#[test]
fn missing_config_exits_nonzero() {
    let output = Command::new(binary_path())
        .arg("gateway")
        .arg("--config")
        .arg("/nonexistent/path.toml")
        .output()
        .expect("failed to execute binary");
    assert!(
        !output.status.success(),
        "missing config should exit with error"
    );
}

#[test]
fn migrate_with_missing_db_exits_nonzero() {
    let output = Command::new(binary_path())
        .arg("migrate")
        .arg("--config")
        .arg("/nonexistent/path.toml")
        .output()
        .expect("failed to execute binary");
    assert!(
        !output.status.success(),
        "missing config for migrate should exit with error"
    );
}
