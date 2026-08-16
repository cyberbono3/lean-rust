//! `lean-rust` binary entry point.

use anyhow::{Context, Result};
use tracing::info;

use lean_cli::cli::{self, Cli};
use lean_cli::commands::{self, Dispatch};
use lean_cli::{node_config, shutdown, startup};

/// Parses the process arguments and hands them to [`run`].
///
/// Kept free of everything else so the argument source is the only thing
/// that separates a real process from the `run` path under test.
#[tokio::main]
async fn main() -> Result<()> {
    run(cli::parse()).await
}

/// Runs one `lean-rust` invocation to completion.
///
/// Either dispatches a subcommand and returns, or builds the devnet node,
/// starts it, and drives it until a shutdown signal arrives. Every step
/// beyond the sequencing lives in `lean_cli`.
///
/// # Errors
///
/// Returns an error if tracing cannot be installed, if a subcommand fails,
/// if the node cannot be configured or constructed, or if starting,
/// awaiting the shutdown signal, or stopping the node fails.
async fn run(cli: Cli) -> Result<()> {
    let _tracing_guard = startup::init_tracing(&cli)?;
    startup::log_startup_config(&cli);
    startup::warn_unwired_flags(&cli);

    match commands::dispatch(&cli)? {
        Dispatch::Handled => return Ok(()),
        Dispatch::RunNode => {}
    }

    let config = node_config::build_devnet_config(&cli).context("build devnet config")?;
    let node = node::new_devnet(config).context("construct devnet node")?;

    node.start().await.context("start node")?;
    info!("node started");

    // The signal error is held, not propagated: a failed wait must still run
    // `node.stop()`. Collapsing this into `shutdown::wait().await?` would skip
    // the shutdown drain on exactly the paths that need it most.
    let signal_result = shutdown::wait().await;
    if signal_result.is_ok() {
        info!("shutdown signal received");
    }
    let stop_result = node.stop().await.context("stop node");

    signal_result.context("wait for shutdown signal")?;
    stop_result
}
