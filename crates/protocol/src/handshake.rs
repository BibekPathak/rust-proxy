//! SOCKS5 negotiation (RFC 1928 §3).
//!
//! This covers the client greeting and the server's method selection only.
//! The username/password sub-negotiation is in [`crate::auth`].

use crate::constants::{method, GREETING_HEADER_LEN, VERSION};
use crate::error::ProtocolError;
use crate::reader::Reader;

/// The client's greeting: a list of acceptable authentication methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greeting {
    /// Authentication methods the client is willing to use.
    pub methods: Vec<u8>,
}

impl Greeting {
    /// Parse a complete greeting frame (header + methods).
    pub fn parse(buf: &[u8]) -> Result<Greeting, ProtocolError> {
        let mut r = Reader::new(buf);
        let ver = r.read_u8()?;
        if ver != VERSION {
            return Err(ProtocolError::BadVersion(ver));
        }
        let nmethods = r.read_u8()? as usize;
        let methods = r.read_bytes(nmethods)?.to_vec();
        Greeting { methods }.finish()
    }

    /// Validate a parsed greeting, bounding the number of methods.
    pub fn finish(self) -> Result<Greeting, ProtocolError> {
        if self.methods.is_empty() || self.methods.len() > 255 {
            return Err(ProtocolError::Unsupported(0x00));
        }
        Ok(self)
    }

    /// The total number of wire bytes of `buf[..]` that a greeting occupies,
    /// based on whether the client declared any methods.
    pub fn frame_len(buf: &[u8]) -> Result<usize, ProtocolError> {
        if buf.len() < GREETING_HEADER_LEN {
            return Err(ProtocolError::Truncated);
        }
        let nmethods = buf[1] as usize;
        Ok(GREETING_HEADER_LEN + nmethods)
    }
}

/// The server's selection of an authentication method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSelection {
    pub method: u8,
}

impl MethodSelection {
    /// Build the 2-byte selection frame `VER METHOD`.
    pub fn to_bytes(self) -> [u8; 2] {
        [VERSION, self.method]
    }

    /// Convenience for selecting "no authentication".
    pub fn no_auth() -> Self {
        Self {
            method: method::NO_AUTH,
        }
    }

    /// Convenience for selecting username/password authentication.
    pub fn user_pass() -> Self {
        Self {
            method: method::USER_PASS,
        }
    }

    /// Convenience for rejecting the negotiation.
    pub fn no_acceptable_method() -> Self {
        Self {
            method: method::NO_ACCEPTABLE,
        }
    }
}

/// Decide which method to select given the client's offered methods and the
/// server's policy.
///
/// Priorities: username/password is preferred when enabled; otherwise anonymous
/// access is used. If none of the client's offered methods match the policy,
/// [`ProtocolError::NoAcceptableMethod`] is returned so the caller can reply
/// with `NO_ACCEPTABLE_METHOD`.
pub fn negotiate(
    greeting: &Greeting,
    allow_user_pass: bool,
    allow_no_auth: bool,
) -> MethodSelection {
    let offered = |m| greeting.methods.contains(&m);
    if allow_no_auth && offered(method::NO_AUTH) {
        return MethodSelection::no_auth();
    }
    if allow_user_pass && offered(method::USER_PASS) {
        return MethodSelection::user_pass();
    }
    MethodSelection::no_acceptable_method()
}
