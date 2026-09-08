//! Proxy node agent.
//!
//! The node agent registers itself with the control plane, sends periodic
//! heartbeats, and listens for chained SOCKS5 CONNECT requests from the
//! gateway. Each connection is proxied to the actual target, with byte
//! accounting reported back to the control plane.

pub mod cp_client;
pub mod error;
pub mod handler;
pub mod metrics;
pub mod server;
