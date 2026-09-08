//! Integration tests for the SOCKS5 gateway.
//!
//! Each test spins up a mock SOCKS5 "node" server and an HTTP target, then
//! runs the gateway against them to exercise the full chain.

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use rustproxy_common::config::GatewayConfig;
use rustproxy_control_plane::api::AppState;
use rustproxy_control_plane::health::HealthPolicy;
use rustproxy_control_plane::registry::{
    migrate, Heartbeat, RegisterRequest, SqliteNodeRepository,
};
use rustproxy_control_plane::service::ControlPlaneService;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

// ── helpers ────────────────────────────────────────────────────────────────

async fn cp_pool() -> (std::sync::Arc<ControlPlaneService>, SqlitePool) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    migrate(&pool).await.unwrap();
    let s = std::sync::Arc::new(ControlPlaneService::from_repo_sql(
        SqliteNodeRepository::new(pool.clone()),
        HealthPolicy::default(),
    ));
    (s, pool)
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

/// A mock SOCKS5 node that accepts CONNECT requests and forwards to the
/// real target server (identified by its address).
async fn start_mock_node(target_addr: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((mut gw_stream, _)) = listener.accept().await {
                let target = target_addr;
                tokio::spawn(async move {
                    mock_node_session(&mut gw_stream, target).await;
                });
            }
        }
    });
    addr
}

/// Handle one gateway→node SOCKS5 session.
async fn mock_node_session(gw: &mut TcpStream, target: SocketAddr) {
    // 1. Read greeting
    let mut hdr = [0u8; 2];
    if gw.read_exact(&mut hdr).await.is_err() {
        return;
    }
    let nmethods = hdr[1] as usize;
    let mut methods = vec![0u8; nmethods];
    let _ = gw.read_exact(&mut methods).await;

    // 2. Reply: NO_AUTH
    let _ = gw.write_all(&[5, 0]).await;

    // 3. Read CONNECT request: VER CMD RSV ATYP ...
    let mut req_hdr = [0u8; 4];
    if gw.read_exact(&mut req_hdr).await.is_err() {
        return;
    }
    let atyp = req_hdr[3];
    let addr_len = match atyp {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            let _ = gw.read_exact(&mut len).await;
            1 + len[0] as usize + 2
        }
        _ => return,
    };
    let mut addr_buf = vec![0u8; addr_len];
    let _ = gw.read_exact(&mut addr_buf).await;

    // 4. Connect to target
    let mut target_stream = match TcpStream::connect(target).await {
        Ok(s) => s,
        Err(_) => {
            let _ = gw.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return;
        }
    };

    // 5. Reply success
    let reply = [5, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    let _ = gw.write_all(&reply).await;

    // 6. Splice
    let _ = tokio::io::copy_bidirectional(gw, &mut target_stream).await;
}

/// A SOCKS5 client that negotiates NO_AUTH + CONNECT to a target.
async fn socks5_connect(
    gw_addr: SocketAddr,
    target: SocketAddr,
    connect_timeout: Duration,
) -> std::io::Result<TcpStream> {
    let mut stream = tokio::time::timeout(connect_timeout, TcpStream::connect(gw_addr))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))?
        .map_err(|e| std::io::Error::new(e.kind(), e.to_string()))?;

    // Greeting: NO_AUTH
    stream.write_all(&[5, 1, 0]).await?;
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await?;
    assert_eq!(sel[1], 0x00, "gateway should agree to NO_AUTH");

    // CONNECT to target — extract IPv4 octets
    let octets = match target.ip() {
        std::net::IpAddr::V4(v4) => v4.octets(),
        _ => panic!("only IPv4 supported in test"),
    };
    let port = target.port().to_be_bytes();
    let connect = [
        5, 0x01, 0x00, // VER CMD RSV
        0x01, // ATYP IPv4
        octets[0], octets[1], octets[2], octets[3], port[0], port[1],
    ];
    stream.write_all(&connect).await?;
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await?;
    assert_eq!(
        reply[1], 0x00,
        "CONNECT should succeed (got {:#04x})",
        reply[1]
    );
    Ok(stream)
}

// ── tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn end_to_end_proxy_chain() {
    let target = start_http_target().await;
    let node = start_mock_node(target).await;
    let (svc, _pool) = cp_pool().await;

    // Register the mock node.
    svc.register(&RegisterRequest {
        node_id: "n1".into(),
        address: node.to_string(),
        max_connections: 100,
        bandwidth_limit: 10_000_000,
    })
    .await
    .unwrap();
    // Heartbeat to make it Healthy.
    svc.heartbeat(&Heartbeat {
        node_id: "n1".into(),
        active_connections: 0,
        bytes_up: 0,
        bytes_down: 0,
        latency_ms: 1,
        available_bandwidth: 10_000_000,
    })
    .await
    .unwrap();

    // Start control plane API on a random port.
    let cp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cp_addr = cp_listener.local_addr().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let sd = shutdown.clone();
    tokio::spawn(async move {
        rustproxy_control_plane::server::run(
            AppState {
                service: svc.clone(),
                auth: rustproxy_control_plane::Auth::None,
            },
            cp_listener,
            Duration::from_secs(30),
            sd,
        )
        .await
        .unwrap();
    });
    sleep(Duration::from_millis(100)).await;

    // Start gateway on a specific port.
    let free_port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let bind_addr: SocketAddr = format!("127.0.0.1:{free_port}").parse().unwrap();
    let gw_cfg = GatewayConfig {
        bind_addr,
        control_plane_url: format!("http://{cp_addr}"),
        api_token: None,
        socks_username: None,
        socks_password: None,
        max_connections: 100,
        connect_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(10),
        metrics_bind_addr: None,
        logging: Default::default(),
    };
    let gw_client = rustproxy_gateway::proxy_client::ControlPlaneClient::new(
        &gw_cfg.control_plane_url,
        gw_cfg.api_token.clone(),
    );
    let gw = rustproxy_gateway::gateway::Gateway::new(gw_cfg, gw_client);
    let sd2 = shutdown.clone();
    tokio::spawn(async move {
        let _ = gw.run(sd2).await;
    });
    sleep(Duration::from_millis(100)).await;

    // Connect through the gateway and fetch from the target HTTP server.
    let mut stream = socks5_connect(bind_addr, target, Duration::from_secs(5))
        .await
        .expect("SOCKS5 connect failed");
    let req = "GET / HTTP/1.1\r\nHost: target\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = vec![0u8; 4096];
    let n = stream.read(&mut resp).await.unwrap();
    let resp = String::from_utf8_lossy(&resp[..n]);
    assert!(resp.contains("200 OK"), "expected 200 OK, got: {resp}");
    assert!(
        resp.contains("hello from target"),
        "expected body, got: {resp}"
    );

    shutdown.cancel();
}

#[tokio::test]
async fn gateway_returns_error_when_no_node_available() {
    let (svc, _pool) = cp_pool().await;
    // Do NOT register any nodes.

    let cp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cp_addr = cp_listener.local_addr().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let sd = shutdown.clone();
    tokio::spawn(async move {
        let _ = rustproxy_control_plane::server::run(
            AppState {
                service: svc.clone(),
                auth: rustproxy_control_plane::Auth::None,
            },
            cp_listener,
            Duration::from_secs(30),
            sd,
        )
        .await;
    });
    sleep(Duration::from_millis(100)).await;

    let free_port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let bind_addr: SocketAddr = format!("127.0.0.1:{free_port}").parse().unwrap();
    let gw_cfg = GatewayConfig {
        bind_addr,
        control_plane_url: format!("http://{cp_addr}"),
        api_token: None,
        socks_username: None,
        socks_password: None,
        max_connections: 100,
        connect_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(10),
        metrics_bind_addr: None,
        logging: Default::default(),
    };
    let gw_client = rustproxy_gateway::proxy_client::ControlPlaneClient::new(
        &gw_cfg.control_plane_url,
        gw_cfg.api_token.clone(),
    );
    let gw = rustproxy_gateway::gateway::Gateway::new(gw_cfg, gw_client);
    let sd2 = shutdown.clone();
    tokio::spawn(async move {
        let _ = gw.run(sd2).await;
    });
    sleep(Duration::from_millis(100)).await;

    // Connect through gateway and try to send a CONNECT.
    let mut stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(bind_addr))
        .await
        .unwrap()
        .unwrap();
    stream.write_all(&[5, 1, 0]).await.unwrap();
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await.unwrap();
    // Send CONNECT to a dummy target
    stream
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 0, 80])
        .await
        .unwrap();
    let mut reply = [0u8; 10];
    let result = stream.read_exact(&mut reply).await;
    if result.is_ok() {
        // The gateway should reply with HOST_UNREACHABLE (0x04) or GENERAL_FAILURE (0x01)
        assert!(reply[1] != 0x00, "expected non-success reply, got SUCCESS");
    }
    // If the connection resets, that's also acceptable (no node → connection dropped).
    shutdown.cancel();
}
