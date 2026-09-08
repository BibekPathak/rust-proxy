//! Workspace-level end-to-end test.
//!
//! Spawns the real `rustproxy` binary for the control plane, a node, and the
//! gateway as subprocesses, then proxies a request through the full chain:
//! `client -> gateway -> node -> target` and back.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

/// Path to the compiled `rustproxy` binary, provided by Cargo for tests in the
/// crate that defines the binary.
const BIN: &str = env!("CARGO_BIN_EXE_rustproxy");

// ── helpers ────────────────────────────────────────────────────────────────

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
}

/// Write a config to a temp directory, returning its path.
fn write_config(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

/// Spawn a `rustproxy` subprocess with the given arguments.
fn spawn(args: &[&str]) -> Child {
    std::process::Command::new(BIN)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {BIN}: {e}"))
}

/// A simple HTTP server that responds with a fixed body.
async fn start_http_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    let body = "hello from target";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        }
    });
    addr
}

/// A minimal SOCKS5 client: greeting + CONNECT (IPv4) to a target, returning
/// the established stream.
async fn socks5_connect(gw: SocketAddr, target: SocketAddr) -> std::io::Result<TcpStream> {
    let mut stream = TcpStream::connect(gw).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await?;
    assert_eq!(sel[1], 0x00, "gateway should agree to NO_AUTH");

    let ip = match target.ip() {
        std::net::IpAddr::V4(v4) => v4.octets(),
        _ => unreachable!(),
    };
    let port = target.port().to_be_bytes();
    let mut req = vec![5, 0x01, 0x00, 0x01];
    req.extend_from_slice(&ip);
    req.extend_from_slice(&port);
    stream.write_all(&req).await?;
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await?;
    assert_eq!(
        reply[1], 0x00,
        "CONNECT should succeed (got {:#04x})",
        reply[1]
    );
    Ok(stream)
}

// ── the test ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_binary_chain_proxies_request() {
    // Random free ports, distinct for each service.
    let cp_port = free_port();
    let gw_port = free_port();
    let node_port = free_port();

    // Temp dir for configs + SQLite DB.
    let tmp = std::env::temp_dir().join(format!("rustproxy-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let db_path = tmp.join("rustproxy.db");

    let cp_cfg = write_config(
        &tmp,
        "control-plane.toml",
        &format!(
            "bind_addr = \"127.0.0.1:{cp_port}\"\n\
             sqlite_path = \"{}\"\n\
             offline_timeout = \"5s\"\n\
             sweep_interval = \"1s\"\n\
             \n\
             [logging]\n\
             level = \"error\"\n\
             json = false\n",
            db_path.display()
        ),
    );
    let node_cfg = write_config(
        &tmp,
        "node.toml",
        &format!(
            "node_id = \"node-1\"\n\
             control_plane_url = \"http://127.0.0.1:{cp_port}\"\n\
             bind_addr = \"127.0.0.1:{node_port}\"\n\
             advertised_address = \"127.0.0.1:{node_port}\"\n\
             max_connections = 32\n\
             heartbeat_interval = \"1s\"\n\
             \n\
             [logging]\n\
             level = \"error\"\n\
             json = false\n"
        ),
    );
    let gw_cfg = write_config(
        &tmp,
        "gateway.toml",
        &format!(
            "bind_addr = \"127.0.0.1:{gw_port}\"\n\
             control_plane_url = \"http://127.0.0.1:{cp_port}\"\n\
             max_connections = 32\n\
             connect_timeout = \"5s\"\n\
             idle_timeout = \"10s\"\n\
             \n\
             [logging]\n\
             level = \"error\"\n\
             json = false\n"
        ),
    );

    // Start the control plane first so the node's registration succeeds.
    let cp = spawn(&["control-plane", "--config", cp_cfg.to_str().unwrap()]);

    // Ensure processes are cleaned up even on failure.
    struct Kill(Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }
    let _cp = Kill(cp);

    // Wait for the control plane HTTP API to come up before starting the node.
    let cp_addr: SocketAddr = format!("127.0.0.1:{cp_port}").parse().unwrap();
    let mut cp_ready = false;
    for _ in 0..100 {
        if let Ok(out) = std::process::Command::new("curl")
            .arg("-fsS")
            .arg(format!("http://{cp_addr}/health"))
            .output()
        {
            if out.status.success() {
                cp_ready = true;
                break;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(cp_ready, "control plane did not become ready in time");

    // Now start the node and the gateway.
    let node = spawn(&["node", "--config", node_cfg.to_str().unwrap()]);
    let gw = spawn(&["gateway", "--config", gw_cfg.to_str().unwrap()]);
    let _node = Kill(node);
    let _gw = Kill(gw);

    // Start an HTTP target the node can connect to.
    let target = start_http_target().await;

    // Wait until the control plane reports a healthy node.
    let mut registered = false;
    for _ in 0..100 {
        if let Ok(out) = std::process::Command::new("curl")
            .arg("-fsS")
            .arg(format!("http://{cp_addr}/nodes"))
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if text.contains("healthy") {
                    registered = true;
                    break;
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(registered, "node did not become healthy in time");

    // Proxy a request through the whole chain: client -> gateway -> node -> target.
    let gw_addr: SocketAddr = format!("127.0.0.1:{gw_port}").parse().unwrap();
    let mut stream = socks5_connect(gw_addr, target)
        .await
        .expect("SOCKS5 failed");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: target\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = vec![0u8; 4096];
    let n = stream.read(&mut resp).await.unwrap();
    let resp = String::from_utf8_lossy(&resp[..n]);
    assert!(resp.contains("200 OK"), "expected 200 OK, got: {resp}");
    assert!(
        resp.contains("hello from target"),
        "expected target body, got: {resp}"
    );

    // Cleanup: drop process handles (kills them via Drop) and remove the temp dir.
    drop(_cp);
    drop(_node);
    drop(_gw);
    std::thread::sleep(Duration::from_millis(200));
    let _ = std::fs::remove_dir_all(&tmp);
    std::thread::sleep(Duration::from_millis(200));
}
