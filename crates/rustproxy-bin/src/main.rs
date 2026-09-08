//! RustProxy unified binary.
//!
//! Dispatches to the gateway, node, control-plane, or migrate subcommand
//! based on CLI arguments.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use rustproxy_common::cli::{Cli, Command};
use rustproxy_common::config;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Gateway(args) => {
            let cfg: rustproxy_common::config::GatewayConfig = config::load(&args.config)?;
            let log_level = args.log_level.unwrap_or(cfg.logging.level);
            rustproxy_common::logging::init(log_level, cfg.logging.json);

            let shutdown = CancellationToken::new();
            let sd = shutdown.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                info!("received Ctrl-C, shutting down");
                sd.cancel();
            });

            info!("starting SOCKS5 gateway on {}", cfg.bind_addr);
            let client = rustproxy_gateway::proxy_client::ControlPlaneClient::new(
                &cfg.control_plane_url,
                cfg.api_token.clone(),
            );
            let gw = rustproxy_gateway::gateway::Gateway::new(cfg, client);
            gw.run(shutdown).await
        }

        Command::Node(args) => {
            let cfg: rustproxy_common::config::NodeConfig = config::load(&args.config)?;
            let log_level = args.log_level.unwrap_or(cfg.logging.level);
            rustproxy_common::logging::init(log_level, cfg.logging.json);

            let shutdown = CancellationToken::new();
            let sd = shutdown.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                info!("received Ctrl-C, shutting down");
                sd.cancel();
            });

            info!("starting node agent {} on {}", cfg.node_id, cfg.bind_addr);
            let node = rustproxy_node::server::NodeServer::new(cfg);
            node.run(shutdown).await
        }

        Command::ControlPlane(args) => {
            let cfg: rustproxy_common::config::ControlPlaneConfig = config::load(&args.config)?;
            let log_level = args.log_level.unwrap_or(cfg.logging.level);
            rustproxy_common::logging::init(log_level, cfg.logging.json);

            let shutdown = CancellationToken::new();
            let sd = shutdown.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                info!("received Ctrl-C, shutting down");
                sd.cancel();
            });

            info!("starting control plane on {}", cfg.bind_addr);

            // Open SQLite and run migrations.
            let pool = sqlx::SqlitePool::connect(&cfg.sqlite_path.to_string_lossy()).await?;
            rustproxy_control_plane::registry::migrate(&pool).await?;

            let repo = Arc::new(rustproxy_control_plane::registry::SqliteNodeRepository::new(pool));
            let service = Arc::new(rustproxy_control_plane::service::ControlPlaneService::new(
                repo,
                rustproxy_control_plane::HealthPolicy::default(),
            ));

            let state = rustproxy_control_plane::server::app_state(service, cfg.api_token.clone());
            let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;
            rustproxy_control_plane::server::run(state, listener, cfg.sweep_interval, shutdown)
                .await
        }

        Command::Migrate(args) => {
            // For migrate, we only need the SQLite path from the config.
            // We load as ControlPlaneConfig to extract sqlite_path.
            let cfg: rustproxy_common::config::ControlPlaneConfig = config::load(&args.config)?;
            let log_level = args.log_level.unwrap_or(cfg.logging.level);
            rustproxy_common::logging::init(log_level, cfg.logging.json);

            info!("applying migrations to {}", cfg.sqlite_path.display());
            let pool = sqlx::SqlitePool::connect(&cfg.sqlite_path.to_string_lossy()).await?;
            rustproxy_control_plane::registry::migrate(&pool).await?;
            info!("migrations applied successfully");
            Ok(())
        }
    }
}
