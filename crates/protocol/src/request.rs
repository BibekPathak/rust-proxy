//! SOCKS5 client request parsing (RFC 1928 §4).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::constants::{atyp, command, REQUEST_HEADER_LEN, VERSION};
use crate::error::ProtocolError;
use crate::host::{Host, SocksAddr};
use crate::reader::Reader;

/// A SOCKS5 command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Connect,
    Bind,
    UdpAssociate,
}

impl Command {
    pub fn from_u8(v: u8) -> Result<Command, ProtocolError> {
        match v {
            command::CONNECT => Ok(Command::Connect),
            command::BIND => Ok(Command::Bind),
            command::UDP_ASSOCIATE => Ok(Command::UdpAssociate),
            other => Err(ProtocolError::Unsupported(other)),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Command::Connect => command::CONNECT,
            Command::Bind => command::BIND,
            Command::UdpAssociate => command::UDP_ASSOCIATE,
        }
    }
}

/// A parsed SOCKS5 request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub command: Command,
    pub addr: SocksAddr,
}

impl Request {
    /// Parse a complete request frame (header + address + port).
    ///
    /// Returns the parsed request together with the total number of bytes
    /// consumed (`4 + address/port length`), which the transport layer uses
    /// to read exactly one frame before calling this.
    pub fn parse(buf: &[u8]) -> Result<(Request, usize), ProtocolError> {
        let mut r = Reader::new(buf);
        let ver = r.read_u8()?;
        if ver != VERSION {
            return Err(ProtocolError::BadVersion(ver));
        }
        let cmd = r.read_u8()?;
        let command = Command::from_u8(cmd)?;
        // RSV must be 0x00; some clients send garbage here, so we accept it
        // but require the command to be recognised.
        let _rsv = r.read_u8()?;
        let atyp = r.read_u8()?;

        let host = match atyp {
            atyp::IPV4 => {
                let b = r.read_bytes(4)?;
                let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
                Host::Ip(IpAddr::V4(ip))
            }
            atyp::IPV6 => {
                let b = r.read_bytes(16)?;
                let mut octets = [0u8; 16];
                octets.copy_from_slice(b);
                let ip = Ipv6Addr::from(octets);
                Host::Ip(IpAddr::V6(ip))
            }
            atyp::DOMAIN => {
                let len = r.read_u8()? as usize;
                if len == 0 || len > 255 {
                    return Err(ProtocolError::Unsupported(atyp::DOMAIN));
                }
                let name = r.read_bytes(len)?;
                let name = String::from_utf8(name.to_vec())
                    .map_err(|_| ProtocolError::Unsupported(atyp::DOMAIN))?;
                Host::Domain(name)
            }
            other => return Err(ProtocolError::Unsupported(other)),
        };

        let port = r.read_u16()?;
        let addr = SocksAddr { host, port };
        Ok((Request { command, addr }, r.position()))
    }

    /// The address+port portion of the request without the 4-byte header.
    pub fn addr_bytes(&self) -> Vec<u8> {
        self.addr.to_vec()
    }

    /// Serialize a full request frame: `VER CMD RSV ATYP DST.ADDR DST.PORT`.
    ///
    /// Used by clients (e.g. the gateway chaining to a node) to build a
    /// `CONNECT` request on the wire.
    pub fn to_bytes(&self) -> Vec<u8> {
        let addr = self.addr.to_vec();
        let mut buf = Vec::with_capacity(REQUEST_HEADER_LEN + addr.len());
        buf.push(VERSION);
        buf.push(self.command.to_u8());
        buf.push(0x00); // RSV
        buf.push(self.addr.atyp());
        buf.extend_from_slice(&addr);
        buf
    }
}

/// Compute the length (in bytes) of the address+port portion of a request
/// given its `ATYP` and, for domain names, the 1-byte length.
///
/// This lets the transport layer know how many bytes to read after the fixed
/// 4-byte header. Returns `None` for unsupported address types (the caller
/// should reply with `ADDRESS_TYPE_NOT_SUPPORTED`).
pub fn addr_len(atyp: u8, domain_len: Option<u8>) -> Result<usize, ProtocolError> {
    match atyp {
        atyp::IPV4 => Ok(4 + 2),
        atyp::IPV6 => Ok(16 + 2),
        atyp::DOMAIN => {
            let len = domain_len.unwrap_or(0) as usize;
            if len == 0 || len > 255 {
                return Err(ProtocolError::Unsupported(atyp::DOMAIN));
            }
            Ok(1 + len + 2)
        }
        other => Err(ProtocolError::Unsupported(other)),
    }
}

/// The size in bytes of the fixed request header.
pub const fn request_header_len() -> usize {
    REQUEST_HEADER_LEN
}
