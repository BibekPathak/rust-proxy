//! Protocol-specific errors.

/// Errors raised while parsing or building SOCKS5 messages.
///
/// This type is separate from [`rustproxy_common::Error`] so that callers can
/// map each failure to the correct SOCKS5 reply code without string matching.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The leading version byte was not `0x05` (or not the auth version).
    #[error("unexpected version byte {0:#04x}")]
    BadVersion(u8),

    /// The client requested a negotiation or address type we do not support.
    #[error("unsupported code {0:#04x}")]
    Unsupported(u8),

    /// The message ended before all declared fields were present.
    #[error("message is truncated")]
    Truncated,

    /// An RFC 1929 credential contained an invalid length.
    #[error("invalid credential length")]
    InvalidCredentialLength,

    /// No negotiation method acceptable to the server was offered.
    #[error("no acceptable authentication method")]
    NoAcceptableMethod,
}

impl ProtocolError {
    /// The position of the first byte of a message is not valid to decode.
    /// Helpers to derive this mapping live in the gateway layer where the
    /// negotiation context is known.
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated)
    }
}
