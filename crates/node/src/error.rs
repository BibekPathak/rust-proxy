//! Node agent error types.

/// Errors encountered by the node agent.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// A SOCKS5 protocol violation.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A handshake or proxy operation exceeded its timeout.
    #[error("timed out: {0}")]
    Timeout(String),

    /// A network I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The upstream relay was terminated.
    #[error("relay error: {0}")]
    Relay(String),
}

pub type Result<T> = std::result::Result<T, NodeError>;
