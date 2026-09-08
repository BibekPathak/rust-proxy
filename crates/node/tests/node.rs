//! Integration tests for the node agent.

use std::time::Duration;

use rustproxy_node::handler::{handle_connection, NodeContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::sleep;

// ── helpers ────────────────────────────────────────────────────────────────

/// A simple TCP server that echoes everything back.
async fn start_echo_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        if stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }
    });
    addr
}

/// A simple TCP server that responds with a fixed HTTP response.
async fn start_http_server() -> std::net::SocketAddr {
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

/// Start a node agent on a random port, returning (bind_addr, shutdown).
async fn start_node(
    auth: Option<(String, String)>,
) -> (std::net::SocketAddr, tokio_util::sync::CancellationToken) {
    let shutdown = tokio_util::sync::CancellationToken::new();
    let sd = shutdown.clone();

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _ = addr_tx.send(addr);

        let ctx = NodeContext {
            auth,
            handshake_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(10),
        };
        loop {
            tokio::select! {
                r = listener.accept() => {
                    if let Ok((stream, _)) = r {
                        let ctx = ctx.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, ctx).await;
                        });
                    }
                }
                _ = sd.cancelled() => break,
            }
        }
    });
    let addr = addr_rx.await.unwrap();
    sleep(Duration::from_millis(50)).await;
    (addr, shutdown)
}

/// Send a SOCKS5 greeting + CONNECT through the given stream.
async fn socks5_handshake(
    stream: &mut TcpStream,
    target: std::net::SocketAddr,
) -> std::io::Result<()> {
    // Greeting: NO_AUTH
    stream.write_all(&[5, 1, 0]).await?;
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await?;
    assert_eq!(sel[1], 0x00, "node should agree to NO_AUTH");

    // CONNECT to target
    let octets = match target.ip() {
        std::net::IpAddr::V4(v4) => v4.octets(),
        _ => panic!("only IPv4 supported in test"),
    };
    let port = target.port().to_be_bytes();
    let connect = [
        5, 0x01, 0x00, 0x01, // VER CMD RSV ATYP IPv4
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
    Ok(())
}

// ── tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn node_relay_echo() {
    let target = start_echo_server().await;
    let (node_addr, shutdown) = start_node(None).await;

    let mut stream = TcpStream::connect(node_addr).await.unwrap();
    socks5_handshake(&mut stream, target).await.unwrap();

    // Send data and verify it comes back
    let msg = b"hello node";
    stream.write_all(msg).await.unwrap();
    let mut resp = vec![0u8; msg.len() + 10];
    let n = stream.read(&mut resp).await.unwrap();
    assert_eq!(&resp[..n], msg);

    shutdown.cancel();
}

#[tokio::test]
async fn node_relay_http() {
    let target = start_http_server().await;
    let (node_addr, shutdown) = start_node(None).await;

    let mut stream = TcpStream::connect(node_addr).await.unwrap();
    socks5_handshake(&mut stream, target).await.unwrap();

    // Send HTTP request
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
async fn node_auth_required_rejects_no_auth() {
    let _target = start_echo_server().await;
    let (node_addr, shutdown) = start_node(Some(("user".into(), "pass".into()))).await;

    let mut stream = TcpStream::connect(node_addr).await.unwrap();
    // Send greeting with NO_AUTH only
    stream.write_all(&[5, 1, 0]).await.unwrap();
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await.unwrap();
    // Should get NO_ACCEPTABLE_METHOD
    assert_eq!(
        sel[1], 0xFF,
        "should reject when auth required but client offers no-auth only"
    );

    shutdown.cancel();
}

#[tokio::test]
async fn node_auth_required_accepts_valid_creds() {
    let target = start_echo_server().await;
    let (node_addr, shutdown) = start_node(Some(("user".into(), "pass".into()))).await;

    let mut stream = TcpStream::connect(node_addr).await.unwrap();

    // Greeting: offer USER_PASS
    stream.write_all(&[5, 1, 2]).await.unwrap();
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel[1], 0x02, "should select USER_PASS");

    // Send username/password (RFC 1929): VER=1, ULEN, UNAME, PLEN, PASSWD
    let user = b"user";
    let pass = b"pass";
    let auth_frame = [
        1,
        user.len() as u8,
        user[0],
        user[1],
        user[2],
        user[3],
        pass.len() as u8,
        pass[0],
        pass[1],
        pass[2],
        pass[3],
    ];
    stream.write_all(&auth_frame).await.unwrap();
    let mut auth_resp = [0u8; 2];
    stream.read_exact(&mut auth_resp).await.unwrap();
    assert_eq!(auth_resp[1], 0x00, "auth should succeed");

    // Now send CONNECT directly (greeting and auth are already done)
    let octets = match target.ip() {
        std::net::IpAddr::V4(v4) => v4.octets(),
        _ => panic!("only IPv4"),
    };
    let port = target.port().to_be_bytes();
    let connect = [
        5, 0x01, 0x00, 0x01, octets[0], octets[1], octets[2], octets[3], port[0], port[1],
    ];
    stream.write_all(&connect).await.unwrap();
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(
        reply[1], 0x00,
        "CONNECT should succeed (got {:#04x})",
        reply[1]
    );

    // Verify data flows
    let msg = b"authenticated";
    stream.write_all(msg).await.unwrap();
    let mut resp = vec![0u8; msg.len() + 10];
    let n = stream.read(&mut resp).await.unwrap();
    assert_eq!(&resp[..n], msg);

    shutdown.cancel();
}

#[tokio::test]
async fn node_auth_required_rejects_bad_creds() {
    let _target = start_echo_server().await;
    let (node_addr, shutdown) = start_node(Some(("user".into(), "pass".into()))).await;

    let mut stream = TcpStream::connect(node_addr).await.unwrap();

    // Greeting: offer USER_PASS
    stream.write_all(&[5, 1, 2]).await.unwrap();
    let mut sel = [0u8; 2];
    stream.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel[1], 0x02);

    // Send wrong credentials
    let auth_frame = [1, 4, b'x', b'y', b'z', b'w', 4, b'a', b'b', b'c', b'd'];
    stream.write_all(&auth_frame).await.unwrap();
    let mut auth_resp = [0u8; 2];
    stream.read_exact(&mut auth_resp).await.unwrap();
    assert_eq!(auth_resp[1], 0x01, "auth should fail with wrong creds");

    shutdown.cancel();
}
