//! Asynchronous SOCKS5 gateway.
//!
//! The gateway is the user-facing SOCKS5 listener.  It accepts client
//! connections, negotiates the SOCKS5 protocol, queries the control plane
//! for the best node, chains a SOCKS5 `CONNECT` through that node, and
//! splices the two byte streams.
//!
//! ```text
//! client ──► gateway ──► node ──► target
//!              ▲
//!              │ control plane
//!           /nodes/select
//! ```

pub mod error;
pub mod gateway;
pub mod handler;
pub mod metrics;
pub mod metrics_server;
pub mod proxy_client;
pub mod relay;
