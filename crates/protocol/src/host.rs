//! Destination address representation for SOCKS5.

use std::fmt;
use std::net::IpAddr;

use crate::constants::atyp;

/// A destination host appearing in a SOCKS5 request.
///
/// Socks DOMAIN addresses are not required to parse as IP addresses, so they
/// are kept separate from [`Host::Ip`] rather than folded into `IpAddr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// An IPv4 or IPv6 literal address.
    Ip(IpAddr),
    /// A DNS name as carried by the `DOMAIN` address type.
    Domain(String),
}

/// A destination `host:port` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksAddr {
    pub host: Host,
    pub port: u16,
}

impl SocksAddr {
    /// The `ATYP` byte that encodes this address.
    pub fn atyp(&self) -> u8 {
        match &self.host {
            Host::Ip(IpAddr::V4(_)) => atyp::IPV4,
            Host::Ip(IpAddr::V6(_)) => atyp::IPV6,
            Host::Domain(_) => atyp::DOMAIN,
        }
    }

    /// The number of wire bytes of the address+port portion of a request
    /// (not including the 4-byte `VER CMD RSV ATYP` header, nor the `ATYP`
    /// byte which lives in that header).
    pub fn wire_len(&self) -> usize {
        match &self.host {
            Host::Ip(IpAddr::V4(_)) => 4 + 2,
            Host::Ip(IpAddr::V6(_)) => 16 + 2,
            Host::Domain(name) => 1 + name.len() + 2,
        }
    }

    /// Serialize the address+port portion (no leading `ATYP` byte) into `out`,
    /// returning how many bytes were written.
    ///
    /// `out` must have at least [`SocksAddr::wire_len`] available; when it
    /// does this is guaranteed to succeed, so the result is the byte count.
    pub fn write(&self, out: &mut [u8]) -> usize {
        let n = self.wire_len();
        // The caller is expected to provide a buffer of exactly `wire_len`;
        // if not, truncating is a programming error we catch in debug builds.
        assert!(out.len() >= n, "address buffer too small");
        let mut pos = 0usize;
        match &self.host {
            Host::Ip(IpAddr::V4(ip)) => {
                out[pos..pos + 4].copy_from_slice(&ip.octets());
                pos += 4;
            }
            Host::Ip(IpAddr::V6(ip)) => {
                out[pos..pos + 16].copy_from_slice(&ip.octets());
                pos += 16;
            }
            Host::Domain(name) => {
                out[pos] = name.len() as u8;
                out[pos + 1..pos + 1 + name.len()].copy_from_slice(name.as_bytes());
                pos += 1 + name.len();
            }
        }
        out[pos] = (self.port >> 8) as u8;
        out[pos + 1] = (self.port & 0xff) as u8;
        n
    }

    /// Serialize the address+port portion (no leading `ATYP` byte) into a
    /// freshly allocated buffer.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut buf = vec![0u8; self.wire_len()];
        self.write(&mut buf);
        buf
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => write!(f, "{ip}"),
            Self::Domain(d) => write!(f, "{d}"),
        }
    }
}

impl fmt::Display for SocksAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}
