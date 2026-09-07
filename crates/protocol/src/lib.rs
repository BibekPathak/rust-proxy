//! Byte-precise SOCKS5 protocol parsing and serialization.
//!
//! This crate is intentionally free of any I/O — it only turns bytes into
//! strongly-typed values and back. Async transport lives in the `gateway` and
//! `node` crates; protocol correctness is unit-tested here in isolation.
//!
//! Supported (RFC 1928, RFC 1929):
//! * negotiation (greeting + method selection)
//! * username/password authentication
//! * the `CONNECT` command
//! * IPv4, IPv6, and domain-name destination addresses
//! * reply serialization

pub mod auth;
pub mod constants;
pub mod error;
pub mod handshake;
pub mod host;
pub mod reply;
pub mod request;

mod reader;

pub use auth::{auth_reply_bytes, UsernamePassword};
pub use error::ProtocolError;
pub use handshake::{negotiate, Greeting, MethodSelection};
pub use host::{Host, SocksAddr};
pub use reply::Reply;
pub use request::{Command, Request};
