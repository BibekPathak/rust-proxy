//! Top-level `rustproxy` command-line interface.
//!
//! The unified binary is `rustproxy <subcommand> --config <file>`. Each
//! subcommand's arguments are defined here so there is a single source of
//! truth for the CLI, mirrored across the service crates.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// RustProxy — a distributed SOCKS5 routing and node orchestration system.
#[derive(Debug, Parser)]
#[command(name = "rustproxy", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// The operations the `rustproxy` binary can perform.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the SOCKS5 gateway that routes client traffic through nodes.
    Gateway(ServiceArgs),
    /// Run a proxy node agent.
    Node(ServiceArgs),
    /// Run the control plane API server.
    ControlPlane(ServiceArgs),
    /// Apply SQLite migrations for a database.
    Migrate(ServiceArgs),
}

/// Arguments shared by every service subcommand.
#[derive(Debug, clap::Args)]
pub struct ServiceArgs {
    /// Path to the TOML configuration file.
    #[arg(long, short, value_name = "FILE")]
    pub config: PathBuf,

    /// Override the configured log level.
    #[arg(long, value_name = "LEVEL")]
    pub log_level: Option<crate::logging::LogLevel>,
}
