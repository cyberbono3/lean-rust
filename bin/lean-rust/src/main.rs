//! `lean-rust` binary entry point.

use anyhow::{Context, Result};
use tracing::info;

use lean_cli::cli::{self, Cli};
use lean_cli::commands::{self, Dispatch};
use lean_cli::{node_config, shutdown, startup};

#[tokio::main]
async fn main() -> Result<()> {
    run(cli::parse()).await
}

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
