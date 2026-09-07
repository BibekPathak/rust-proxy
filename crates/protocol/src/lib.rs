//! Byte-precise SOCKS5 protocol parsing and serialization.
//!
//! This crate is intentionally free of any I/O — it only turns bytes into
//! strongly-typed values and back. Async transport lives in the `gateway` and
//! `node` crates; protocol correctness is unit-tested here in isolation.

pub fn placeholder() -> &'static str {
    "protocol"
}
