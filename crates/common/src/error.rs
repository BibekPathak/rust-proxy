//! Typed error handling shared across RustProxy services.

use std::io;

/// The unified error type for library code in this workspace.
///
/// Services layer their own errors on top of this where useful, but every
/// crate ultimately converges on [`Error`] so that boundaries stay clean.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O error from Tokio or the standard library.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// A serialization/deserialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A malformed or unparseable prototol message.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// An error from the control plane API integration.
    #[error("control plane error: {0}")]
    ControlPlane(String),

    /// The underlying cause is unknown or intentionally opaque.
    #[error("unknown error")]
    Other,
}

/// Convenience alias used throughout the workspace.
pub type Result<T> = std::result::Result<T, Error>;
