//! Per-connection SOCKS5 handler.
//!
//! Each accepted TCP connection is handed to [`handle_connection`], which
//! performs the full client-facing SOCKS5 preamble, selects a remote node,
//! chains a SOCKS5 `CONNECT` to that node, and splices the two byte streams.
//!
//! ```text
//! client → greeting → auth → CONNECT(select) → node → reply to client → relay
//! ```

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::Instrument;
use tracing::{debug, info, warn};

use crate::error::{GatewayError, Result};
use crate::proxy_client::ControlPlaneClient;
use crate::relay;

/// Per-connection context carrying shared resources.
#[derive(Clone)]
pub struct GatewayContext {
    pub client: ControlPlaneClient,
    pub auth: Option<(String, String)>,
    pub connect_timeout: Duration,
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
        .map_err(|_| GatewayError::Timeout("handshake read timed out".into()))?
        .map(|_| ())
        .map_err(GatewayError::Io)
}

async fn write_all_timeout(stream: &mut TcpStream, data: &[u8], deadline: Duration) -> Result<()> {
    timeout(deadline, stream.write_all(data))
        .await
        .map_err(|_| GatewayError::Timeout("handshake write timed out".into()))?
        .map_err(GatewayError::Io)
}

fn reply_bytes(code: u8) -> Vec<u8> {
    rustproxy_protocol::reply::simple_reply(code).to_bytes()
}

/// Best-effort reply write: fire-and-forget, ignoring errors.
async fn try_reply(client: &mut TcpStream, code: u8, deadline: Duration) {
    let _ = write_all_timeout(client, &reply_bytes(code), deadline).await;
}

// ── main entry point ───────────────────────────────────────────────────────

/// Handle a single SOCKS5 client connection.
pub async fn handle_connection(mut client_stream: TcpStream, ctx: GatewayContext) {
    let conn_id = uuid::Uuid::new_v4().to_string();
    let peer = client_stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());

    async {
        crate::metrics::record_connection_opened();

        match run_with_ctx(&mut client_stream, &ctx).await {
            Ok(()) => {
                debug!("connection closed cleanly");
            }
            Err(e) => {
                crate::metrics::record_connection_error();
                warn!(error = %e, "connection failed");
            }
        }

        crate::metrics::record_connection_closed();
    }
    .instrument(tracing::info_span!("conn", %conn_id, %peer))
    .await;
}

/// The real per-connection runner.
async fn run_with_ctx(client: &mut TcpStream, ctx: &GatewayContext) -> Result<()> {
    let dt = ctx.connect_timeout;

    // 1. Greeting
    let greeting = read_greeting(client, dt).await?;
    let allow_no_auth = ctx.auth.is_none();
    let allow_user_pass = ctx.auth.is_some();
    let method = rustproxy_protocol::negotiate(&greeting, allow_user_pass, allow_no_auth);
    write_all_timeout(client, &method.to_bytes(), dt).await?;

    if method.method == rustproxy_protocol::constants::method::NO_ACCEPTABLE {
        return Err(GatewayError::Protocol(
            "no acceptable authentication method".into(),
        ));
    }

    // 2. Authentication (RFC 1929)
    if method.method == rustproxy_protocol::constants::method::USER_PASS {
        handle_user_pass_auth(client, ctx).await?;
    }

    // 3. CONNECT request
    let request = read_request(client, dt).await?;

    if request.command != rustproxy_protocol::Command::Connect {
        try_reply(
            client,
            rustproxy_protocol::constants::reply::COMMAND_NOT_SUPPORTED,
            dt,
        )
        .await;
        return Err(GatewayError::Protocol(
            "unsupported command (only CONNECT is supported)".into(),
        ));
    }

    info!(dest = %request.addr, cmd = ?request.command, "client wants CONNECT");

    // 4. Select a node via the control plane
    let node = match timeout(dt, ctx.client.select_node()).await {
        Err(_) => {
            try_reply(
                client,
                rustproxy_protocol::constants::reply::HOST_UNREACHABLE,
                dt,
            )
            .await;
            return Err(GatewayError::Timeout("select node timed out".into()));
        }
        Ok(Err(e)) => {
            try_reply(
                client,
                rustproxy_protocol::constants::reply::HOST_UNREACHABLE,
                dt,
            )
            .await;
            return Err(e);
        }
        Ok(Ok(n)) => n,
    };

    debug!(node_id = %node.id, addr = %node.forward_address, "selected node");

    // 5. Connect to node and chain a SOCKS5 CONNECT
    let mut node_stream = connect_and_chain(&node.forward_address, &request.addr, dt).await?;

    // 6. Reply success to client
    write_all_timeout(
        client,
        &reply_bytes(rustproxy_protocol::constants::reply::SUCCESS),
        dt,
    )
    .await?;

    // 7. Splice the two byte streams
    let stats = relay::relay(client, &mut node_stream, ctx.idle_timeout, "").await?;
    crate::metrics::record_bytes_up(stats.bytes_up);
    crate::metrics::record_bytes_down(stats.bytes_down);
    info!(
        bytes_up = stats.bytes_up,
        bytes_down = stats.bytes_down,
        "relay finished"
    );
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
        return Err(GatewayError::Protocol("greeting: no methods".into()));
    }
    let mut methods = vec![0u8; nmethods];
    read_exact_timeout(stream, &mut methods, deadline).await?;
    let mut buf = Vec::with_capacity(2 + nmethods);
    buf.extend_from_slice(&hdr);
    buf.extend_from_slice(&methods);
    rustproxy_protocol::Greeting::parse(&buf).map_err(|e| GatewayError::Protocol(e.to_string()))
}

async fn handle_user_pass_auth(stream: &mut TcpStream, ctx: &GatewayContext) -> Result<()> {
    let mut ver = [0u8; 1];
    read_exact_timeout(stream, &mut ver, ctx.connect_timeout).await?;
    if ver[0] != rustproxy_protocol::constants::AUTH_VERSION {
        return Err(GatewayError::Protocol(format!(
            "auth frame version {:#04x}, expected {:#04x}",
            ver[0],
            rustproxy_protocol::constants::AUTH_VERSION,
        )));
    }
    let mut ulen = [0u8; 1];
    read_exact_timeout(stream, &mut ulen, ctx.connect_timeout).await?;
    let ulen = ulen[0] as usize;
    if ulen == 0 || ulen > 255 {
        return Err(GatewayError::Protocol("invalid username length".into()));
    }
    let mut uname = vec![0u8; ulen];
    read_exact_timeout(stream, &mut uname, ctx.connect_timeout).await?;
    let mut plen = [0u8; 1];
    read_exact_timeout(stream, &mut plen, ctx.connect_timeout).await?;
    let plen = plen[0] as usize;
    let mut passwd = vec![0u8; plen];
    read_exact_timeout(stream, &mut passwd, ctx.connect_timeout).await?;

    let username = String::from_utf8(uname)
        .map_err(|_| GatewayError::Protocol("invalid utf-8 in username".into()))?;
    let password = String::from_utf8(passwd)
        .map_err(|_| GatewayError::Protocol("invalid utf-8 in password".into()))?;

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
        ctx.connect_timeout,
    )
    .await?;

    if !ok {
        return Err(GatewayError::Protocol("authentication failed".into()));
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
    // For a domain address the 1-byte length prefix is read here; the address
    // buffer that follows therefore holds name + port (len + 2 bytes), and the
    // length prefix is re-attached when assembling the full frame.
    let (full_addr_len, addr_buf_len, domain_len) =
        if atyp == rustproxy_protocol::constants::atyp::DOMAIN {
            let mut len_byte = [0u8; 1];
            read_exact_timeout(stream, &mut len_byte, deadline).await?;
            let len = len_byte[0];
            let total = rustproxy_protocol::request::addr_len(atyp, Some(len))
                .map_err(|e| GatewayError::Protocol(e.to_string()))?;
            (total, total - 1, Some(len))
        } else {
            let total = rustproxy_protocol::request::addr_len(atyp, None)
                .map_err(|e| GatewayError::Protocol(e.to_string()))?;
            (total, total, None)
        };

    let mut addr_buf = vec![0u8; addr_buf_len];
    read_exact_timeout(stream, &mut addr_buf, deadline).await?;

    let mut full = Vec::with_capacity(4 + full_addr_len);
    full.extend_from_slice(&hdr);
    if let Some(len) = domain_len {
        full.push(len);
    }
    full.extend_from_slice(&addr_buf);

    rustproxy_protocol::Request::parse(&full)
        .map(|(req, _)| req)
        .map_err(|e| GatewayError::Protocol(e.to_string()))
}

async fn connect_and_chain(
    node_address: &str,
    dest: &rustproxy_protocol::SocksAddr,
    deadline: Duration,
) -> Result<TcpStream> {
    let mut node_stream = timeout(deadline, TcpStream::connect(node_address))
        .await
        .map_err(|_| GatewayError::Timeout(format!("node connect timed out ({node_address})")))?
        .map_err(GatewayError::Io)?;

    let greeting = [rustproxy_protocol::constants::VERSION, 1, 0];
    write_all_timeout(&mut node_stream, &greeting, deadline).await?;

    let mut sel = [0u8; 2];
    read_exact_timeout(&mut node_stream, &mut sel, deadline).await?;
    if sel[0] != rustproxy_protocol::constants::VERSION {
        return Err(GatewayError::Protocol(format!(
            "node selection version {:#04x}",
            sel[0],
        )));
    }
    if sel[1] != rustproxy_protocol::constants::method::NO_AUTH {
        return Err(GatewayError::Protocol(format!(
            "node rejected NO_AUTH (got {:#04x})",
            sel[1],
        )));
    }

    let request = rustproxy_protocol::Request {
        command: rustproxy_protocol::Command::Connect,
        addr: dest.clone(),
    };
    write_all_timeout(&mut node_stream, &request.to_bytes(), deadline).await?;

    let mut reply_buf = [0u8; 4];
    read_exact_timeout(&mut node_stream, &mut reply_buf, deadline).await?;
    if reply_buf[0] != rustproxy_protocol::constants::VERSION {
        return Err(GatewayError::Protocol("node reply version mismatch".into()));
    }
    let node_atyp = reply_buf[3];
    let node_addr_len = match node_atyp {
        rustproxy_protocol::constants::atyp::IPV4 => 4 + 2,
        rustproxy_protocol::constants::atyp::IPV6 => 16 + 2,
        rustproxy_protocol::constants::atyp::DOMAIN => {
            let mut dlen = [0u8; 1];
            read_exact_timeout(&mut node_stream, &mut dlen, deadline).await?;
            1 + dlen[0] as usize + 2
        }
        _ => return Err(GatewayError::Protocol("unknown node reply ATYP".into())),
    };
    let mut node_addr = vec![0u8; node_addr_len];
    read_exact_timeout(&mut node_stream, &mut node_addr, deadline).await?;

    let rep = reply_buf[1];
    if rep != rustproxy_protocol::constants::reply::SUCCESS {
        return Err(GatewayError::Relay(format!(
            "node CONNECT failed (reply {rep:#04x})"
        )));
    }

    Ok(node_stream)
}
