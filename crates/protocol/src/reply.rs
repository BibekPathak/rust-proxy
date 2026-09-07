//! SOCKS5 server replies (RFC 1928 §6).

use crate::constants::VERSION;
use crate::host::SocksAddr;
use crate::reader::Reader;

/// A SOCKS5 reply from the server to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The reply code (see [`crate::constants::reply`]).
    pub code: u8,
    /// The bound address reported to the client.
    pub bind_addr: SocksAddr,
}

impl Reply {
    /// Serialize a reply into a fresh buffer.
    ///
    /// Layout: `VER REP RSV ATYP BND.ADDR BND.PORT`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let addr = self.bind_addr.to_vec();
        let mut buf = Vec::with_capacity(4 + addr.len());
        buf.push(VERSION);
        buf.push(self.code);
        buf.push(0x00); // RSV
        buf.push(self.bind_addr.atyp());
        buf.extend_from_slice(&addr);
        buf
    }

    /// Parse a complete reply frame.
    pub fn parse(buf: &[u8]) -> Result<Reply, crate::error::ProtocolError> {
        use crate::constants::atyp;
        use crate::error::ProtocolError;

        let mut r = Reader::new(buf);
        let ver = r.read_u8()?;
        if ver != VERSION {
            return Err(ProtocolError::BadVersion(ver));
        }
        let code = r.read_u8()?;
        let _rsv = r.read_u8()?;
        let atyp = r.read_u8()?;

        let host = match atyp {
            atyp::IPV4 => {
                let b = r.read_bytes(4)?;
                crate::host::Host::Ip(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    b[0], b[1], b[2], b[3],
                )))
            }
            atyp::IPV6 => {
                let b = r.read_bytes(16)?;
                let mut o = [0u8; 16];
                o.copy_from_slice(b);
                crate::host::Host::Ip(std::net::IpAddr::V6(std::net::Ipv6Addr::from(o)))
            }
            atyp::DOMAIN => {
                let len = r.read_u8()? as usize;
                let name = r.read_bytes(len)?;
                crate::host::Host::Domain(
                    String::from_utf8(name.to_vec())
                        .map_err(|_| ProtocolError::Unsupported(atyp))?,
                )
            }
            other => return Err(ProtocolError::Unsupported(other)),
        };

        let port = r.read_u16()?;
        Ok(Reply {
            code,
            bind_addr: SocksAddr { host, port },
        })
    }
}

/// Build a reply with a conventional "no special bind address" (0.0.0.0:0),
/// used when the server does not need to report a meaningful bound endpoint.
pub fn simple_reply(code: u8) -> Reply {
    Reply {
        code,
        bind_addr: SocksAddr {
            host: crate::host::Host::Ip(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
            port: 0,
        },
    }
}

/// Convenience: an accepted reply with the standard empty bind address.
pub fn success() -> Reply {
    simple_reply(crate::constants::reply::SUCCESS)
}
