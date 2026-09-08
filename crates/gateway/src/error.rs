//! Gateway error types.

/// Errors encountered while handling a SOCKS5 client connection.
///
/// These are deliberately coarse: protocol-level failures map to SOCKS5 reply
/// codes where appropriate, and transport failures are logged and the
/// connection dropped.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// A SOCKS5 protocol violation (malformed or unsupported message).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The handshake or a proxy operation exceeded its timeout.
    #[error("timed out: {0}")]
    Timeout(String),

    /// No healthy node was available to route the connection through.
    #[error("no node available")]
    NoNode,

    /// A network I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The upstream relay was terminated.
    #[error("relay error: {0}")]
    Relay(String),
}

pub type Result<T> = std::result::Result<T, GatewayError>;
