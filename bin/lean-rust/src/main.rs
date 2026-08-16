//! `lean-rust` binary entry point.

use anyhow::{Context, Result};
use tracing::info;

use lean_cli::cli::Cli;
use lean_cli::commands::Dispatch;

#[tokio::main]
async fn main() -> Result<()> {
    run(lean_cli::cli::parse()).await
}

async fn run(cli: Cli) -> Result<()> {
    let _tracing_guard = lean_cli::startup::init_tracing(&cli)?;
    lean_cli::startup::log_startup_config(&cli);
    lean_cli::startup::warn_unwired_flags(&cli);

    if lean_cli::commands::dispatch(&cli)? == Dispatch::Handled {
        return Ok(());
    }

    let config = lean_cli::node_config::build_devnet_config(&cli).context("build devnet config")?;
    let node = node::new_devnet(config).context("construct devnet node")?;

    node.start().await.context("start node")?;
    info!("node started");

    let signal_result = lean_cli::shutdown::wait().await;
    if signal_result.is_ok() {
        info!("shutdown signal received");
    }
    let stop_result = node.stop().await.context("stop node");

    signal_result.context("wait for shutdown signal")?;
    stop_result
}
