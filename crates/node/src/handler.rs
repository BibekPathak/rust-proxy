//! Per-connection SOCKS5 handler for the node agent.
//!
//! The node receives a chained CONNECT from the gateway:
//!
//! ```text
//! gateway → greeting → auth → CONNECT(target) → node → reply → relay
//! ```

use std::time::Duration;

use tokio::io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::Instrument;
use tracing::{debug, info, warn};

use crate::error::{NodeError, Result};
use crate::metrics;

/// Per-connection context.
#[derive(Clone)]
pub struct NodeContext {
    /// Required username/password; `None` means accept no-auth only.
    pub auth: Option<(String, String)>,
    /// Timeout for handshake reads/writes.
    pub handshake_timeout: Duration,
    /// Idle timeout for the relay phase.
    pub idle_timeout: Duration,
}

// ── helpers ────────────────────────────────────────────────────────────────

async fn read_exact_timeout(
    stream: &mut TcpStream,
    buf: &mut [u8],
    deadline: Duration,
) -> Result<()> {
    timeout(deadline, stream.read_exact(buf))
        .await
        .map_err(|_| NodeError::Timeout("handshake read timed out".into()))?
        .map(|_| ())
        .map_err(NodeError::Io)
}

async fn write_all_timeout(stream: &mut TcpStream, data: &[u8], deadline: Duration) -> Result<()> {
    timeout(deadline, stream.write_all(data))
        .await
        .map_err(|_| NodeError::Timeout("handshake write timed out".into()))?
        .map_err(NodeError::Io)
}

fn reply_bytes(code: u8) -> Vec<u8> {
    rustproxy_protocol::reply::simple_reply(code).to_bytes()
}

async fn try_reply(client: &mut TcpStream, code: u8, deadline: Duration) {
    let _ = write_all_timeout(client, &reply_bytes(code), deadline).await;
}

// ── main entry point ───────────────────────────────────────────────────────

/// Handle a single gateway → node SOCKS5 connection.
pub async fn handle_connection(mut gateway_stream: TcpStream, ctx: NodeContext) {
    let conn_id = uuid::Uuid::new_v4().to_string();
    let peer = gateway_stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());

    async {
        metrics::record_connection_opened();

        match run_with_ctx(&mut gateway_stream, &ctx).await {
            Ok(()) => debug!("node session completed"),
            Err(e) => {
                metrics::record_connection_error();
                warn!(error = %e, "node session failed");
            }
        }

        metrics::record_connection_closed();
    }
    .instrument(tracing::info_span!("node_conn", %conn_id, %peer))
    .await;
}

async fn run_with_ctx(gw: &mut TcpStream, ctx: &NodeContext) -> Result<()> {
    let dt = ctx.handshake_timeout;

    // 1. Read greeting
    let greeting = read_greeting(gw, dt).await?;
    let allow_no_auth = ctx.auth.is_none();
    let allow_user_pass = ctx.auth.is_some();
    let method = rustproxy_protocol::negotiate(&greeting, allow_user_pass, allow_no_auth);
    write_all_timeout(gw, &method.to_bytes(), dt).await?;

    if method.method == rustproxy_protocol::constants::method::NO_ACCEPTABLE {
        return Err(NodeError::Protocol(
            "no acceptable authentication method".into(),
        ));
    }

    // 2. Authentication (RFC 1929)
    if method.method == rustproxy_protocol::constants::method::USER_PASS {
        handle_user_pass_auth(gw, ctx).await?;
    }

    // 3. Read CONNECT request
    let request = read_request(gw, dt).await?;

    if request.command != rustproxy_protocol::Command::Connect {
        try_reply(
            gw,
            rustproxy_protocol::constants::reply::COMMAND_NOT_SUPPORTED,
            dt,
        )
        .await;
        return Err(NodeError::Protocol(
            "unsupported command (only CONNECT is supported)".into(),
        ));
    }

    info!(dest = %request.addr, "gateway wants CONNECT");

    // 4. Connect to the actual target
    let target_addr = request.addr.to_string();
    let mut target_stream = match timeout(dt, TcpStream::connect(&target_addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let code = if e.kind() == std::io::ErrorKind::ConnectionRefused {
                rustproxy_protocol::constants::reply::CONNECTION_REFUSED
            } else {
                rustproxy_protocol::constants::reply::NETWORK_UNREACHABLE
            };
            try_reply(gw, code, dt).await;
            return Err(NodeError::Io(e));
        }
        Err(_) => {
            try_reply(gw, rustproxy_protocol::constants::reply::TTL_EXPIRED, dt).await;
            return Err(NodeError::Timeout("target connect timed out".into()));
        }
    };

    // 5. Reply success to the gateway
    write_all_timeout(
        gw,
        &reply_bytes(rustproxy_protocol::constants::reply::SUCCESS),
        dt,
    )
    .await?;

    // 6. Splice the two byte streams
    let result = timeout(ctx.idle_timeout, copy_bidirectional(gw, &mut target_stream)).await;

    match result {
        Ok(Ok((up, down))) => {
            metrics::record_bytes_up(up);
            metrics::record_bytes_down(down);
            info!(bytes_up = up, bytes_down = down, "relay finished");
        }
        Ok(Err(e)) => {
            debug!(error = %e, "relay io error");
        }
        Err(_) => {
            debug!("relay idle timeout");
        }
    }

    Ok(())
}

// ── internal helpers ────────────────────────────────────────────────────────

async fn read_greeting(
    stream: &mut TcpStream,
    deadline: Duration,
) -> Result<rustproxy_protocol::Greeting> {
    let mut hdr = [0u8; rustproxy_protocol::constants::GREETING_HEADER_LEN];
    read_exact_timeout(stream, &mut hdr, deadline).await?;
    let nmethods = hdr[1] as usize;
    if nmethods == 0 {
        return Err(NodeError::Protocol("greeting: no methods".into()));
    }
    let mut methods = vec![0u8; nmethods];
    read_exact_timeout(stream, &mut methods, deadline).await?;
    let mut buf = Vec::with_capacity(2 + nmethods);
    buf.extend_from_slice(&hdr);
    buf.extend_from_slice(&methods);
    rustproxy_protocol::Greeting::parse(&buf).map_err(|e| NodeError::Protocol(e.to_string()))
}

async fn handle_user_pass_auth(stream: &mut TcpStream, ctx: &NodeContext) -> Result<()> {
    let mut ver = [0u8; 1];
    read_exact_timeout(stream, &mut ver, ctx.handshake_timeout).await?;
    if ver[0] != rustproxy_protocol::constants::AUTH_VERSION {
        return Err(NodeError::Protocol(format!(
            "auth frame version {:#04x}, expected {:#04x}",
            ver[0],
            rustproxy_protocol::constants::AUTH_VERSION,
        )));
    }
    let mut ulen = [0u8; 1];
    read_exact_timeout(stream, &mut ulen, ctx.handshake_timeout).await?;
    let ulen = ulen[0] as usize;
    if ulen == 0 || ulen > 255 {
        return Err(NodeError::Protocol("invalid username length".into()));
    }
    let mut uname = vec![0u8; ulen];
    read_exact_timeout(stream, &mut uname, ctx.handshake_timeout).await?;
    let mut plen = [0u8; 1];
    read_exact_timeout(stream, &mut plen, ctx.handshake_timeout).await?;
    let plen = plen[0] as usize;
    let mut passwd = vec![0u8; plen];
    read_exact_timeout(stream, &mut passwd, ctx.handshake_timeout).await?;

    let username = String::from_utf8(uname)
        .map_err(|_| NodeError::Protocol("invalid utf-8 in username".into()))?;
    let password = String::from_utf8(passwd)
        .map_err(|_| NodeError::Protocol("invalid utf-8 in password".into()))?;

    let ok = match &ctx.auth {
        Some((expected_user, expected_pass)) => {
            &username == expected_user && &password == expected_pass
        }
        None => false,
    };

    let status = if ok {
        rustproxy_protocol::auth::auth_status::SUCCESS
    } else {
        rustproxy_protocol::auth::auth_status::FAILURE
    };
    write_all_timeout(
        stream,
        &rustproxy_protocol::auth_reply_bytes(status),
        ctx.handshake_timeout,
    )
    .await?;

    if !ok {
        return Err(NodeError::Protocol("authentication failed".into()));
    }
    Ok(())
}

async fn read_request(
    stream: &mut TcpStream,
    deadline: Duration,
) -> Result<rustproxy_protocol::Request> {
    let mut hdr = [0u8; rustproxy_protocol::constants::REQUEST_HEADER_LEN];
    read_exact_timeout(stream, &mut hdr, deadline).await?;

    let atyp = hdr[3];
    let full_addr_len = if atyp == rustproxy_protocol::constants::atyp::DOMAIN {
        let mut len_byte = [0u8; 1];
        read_exact_timeout(stream, &mut len_byte, deadline).await?;
        rustproxy_protocol::request::addr_len(atyp, Some(len_byte[0]))
            .map_err(|e| NodeError::Protocol(e.to_string()))?
    } else {
        rustproxy_protocol::request::addr_len(atyp, None)
            .map_err(|e| NodeError::Protocol(e.to_string()))?
    };

    let mut addr_buf = vec![0u8; full_addr_len];
    read_exact_timeout(stream, &mut addr_buf, deadline).await?;

    let mut full = Vec::with_capacity(4 + full_addr_len);
    full.extend_from_slice(&hdr);
    full.extend_from_slice(&addr_buf);

    rustproxy_protocol::Request::parse(&full)
        .map(|(req, _)| req)
        .map_err(|e| NodeError::Protocol(e.to_string()))
}
