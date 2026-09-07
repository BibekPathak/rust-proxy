//! SOCKS5 protocol constants.
//!
//! See [RFC 1928](https://datatracker.ietf.org/doc/html/rfc1928) for the
//! wire protocol and [RFC 1929](https://datatracker.ietf.org/doc/html/rfc1929)
//! for the username/password authentication sub-negotiation.

/// The SOCKS5 protocol version byte.
pub const VERSION: u8 = 0x05;

/// The version byte (0x01) used by the RFC 1929 sub-negotiation.
pub const AUTH_VERSION: u8 = 0x01;

/// The fixed size of the greeting header: `VER NMETHODS`.
pub const GREETING_HEADER_LEN: usize = 2;

/// The fixed size of the request header: `VER CMD RSV ATYP`.
pub const REQUEST_HEADER_LEN: usize = 4;

/// The fixed size of a reply header: `VER REP RSV ATYP`.
pub const REPLY_HEADER_LEN: usize = 4;

/// Authentication methods (RFC 1928 §3).
pub mod method {
    /// No authentication required.
    pub const NO_AUTH: u8 = 0x00;
    /// Username/password authentication (RFC 1929).
    pub const USER_PASS: u8 = 0x02;
    /// No acceptable method was offered.
    pub const NO_ACCEPTABLE: u8 = 0xff;
}

/// Client commands (RFC 1928 §4).
pub mod command {
    /// Establish a TCP connection to the requested destination.
    pub const CONNECT: u8 = 0x01;
    /// Bind the requested destination (unused for plain proxying).
    pub const BIND: u8 = 0x02;
    /// UDP relay association (unused for TCP proxying).
    pub const UDP_ASSOCIATE: u8 = 0x03;
}

/// Address types (RFC 1928 §4).
pub mod atyp {
    /// IPv4 — 4 address bytes followed by a 2-byte port.
    pub const IPV4: u8 = 0x01;
    /// Domain name — a 1-byte length, up to 255 name bytes, then a 2-byte port.
    pub const DOMAIN: u8 = 0x03;
    /// IPv6 — 16 address bytes followed by a 2-byte port.
    pub const IPV6: u8 = 0x04;
}

/// Reply codes (RFC 1928 §6).
pub mod reply {
    pub const SUCCESS: u8 = 0x00;
    pub const GENERAL_FAILURE: u8 = 0x01;
    pub const NOT_ALLOWED: u8 = 0x02;
    pub const NETWORK_UNREACHABLE: u8 = 0x03;
    pub const HOST_UNREACHABLE: u8 = 0x04;
    pub const CONNECTION_REFUSED: u8 = 0x05;
    pub const TTL_EXPIRED: u8 = 0x06;
    pub const COMMAND_NOT_SUPPORTED: u8 = 0x07;
    pub const ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;
}
