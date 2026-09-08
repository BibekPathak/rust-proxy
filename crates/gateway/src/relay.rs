//! Bidirectional TCP relay with idle timeout and byte accounting.
//!
//! Splices two byte streams together (typically a SOCKS5 client connection
//! and the chained node connection) until one side closes, then sends a
//! shutdown to the other.

use std::time::Duration;

use tokio::io::{copy_bidirectional, AsyncRead, AsyncWrite};
use tokio::time::timeout;

/// Byte counts from a completed relay.
#[derive(Debug, Clone, Copy)]
pub struct RelayStats {
    /// Client → node (upstream).
    pub bytes_up: u64,
    /// Node → client (downstream).
    pub bytes_down: u64,
}

/// Splice `client` and `node` together, returning byte counts.
///
/// The relay completes when one side sends EOF.  An `idle_timeout` is
/// applied to the entire relay: if neither side progresses within the
/// window the connection is torn down.
pub async fn relay<A, B>(
    client: &mut A,
    node: &mut B,
    idle_timeout: Duration,
    conn_id: &str,
) -> crate::error::Result<RelayStats>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let result = timeout(idle_timeout, copy_bidirectional(client, node)).await;

    match result {
        Ok(Ok((up, down))) => {
            tracing::trace!(conn_id, up, down, "relay completed");
            Ok(RelayStats {
                bytes_up: up,
                bytes_down: down,
            })
        }
        Ok(Err(e)) => {
            tracing::debug!(conn_id, error = %e, "relay io error");
            Err(crate::error::GatewayError::Relay(e.to_string()))
        }
        Err(_) => {
            tracing::debug!(conn_id, "relay idle timeout");
            Err(crate::error::GatewayError::Timeout(
                "relay idle timeout".into(),
            ))
        }
    }
}
