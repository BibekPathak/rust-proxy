//! RustProxy shared utilities.
//!
//! This crate contains the cross-cutting concerns shared by the `gateway`,
//! `node`, and `control-plane` crates:
//!
//! * [`error`] — typed error handling.
//! * [`logging`] — `tracing`-based structured logging setup.
//! * [`cli`] — the top-level `rustproxy` command-line interface.
//! * [`config`] — per-service configuration models.
//!
//! Keeping these conventions in one place avoids duplicated, drifting logic
//! across the service crates.

pub mod cli;
pub mod config;
pub mod error;
pub mod logging;

pub use error::{Error, Result};
