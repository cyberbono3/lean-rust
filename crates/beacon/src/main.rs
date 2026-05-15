//! `lean-beacon` binary entry point.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use runtime_core::{FileSink, NodeConfig, TracingGuard};
use runtime_p2p::HostOptions;
use tracing::{info, warn};

use crate::cli::{Cli, Command};

mod cli;
mod genesis;
mod keygen;

const AGENT_VERSION: &str = concat!("lean-beacon/", env!("CARGO_PKG_VERSION"));
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:5052";
const DEFAULT_IDENTITY_PATH: &str = "p2p_priv_key";
const DEFAULT_LOG_PREFIX: &str = "lean-beacon";
const DEFAULT_METRICS_ADDR: &str = "127.0.0.1:9090";
const DEFAULT_VALIDATORS_PATH: &str = "crates/runtime/duties/tests/fixtures/validators.yaml";

#[tokio::main]
async fn main() -> Result<()> {
    run(Cli::parse()).await
}

async fn run(cli: Cli) -> Result<()> {
    let _tracing_guard = init_tracing(&cli)?;

    if let Some(Command::GeneratePrivateKey { output_path }) = &cli.command {
        let peer_id = keygen::generate_and_write(output_path).context("generate private key")?;
        info!(%peer_id, path = %output_path.display(), "generated libp2p private key");
        return Ok(());
    }

    let config = build_devnet_config(&cli).context("build devnet config")?;
    let node = node::new_devnet(config).context("construct devnet node")?;

    node.start().await.context("start node")?;
    info!("node started");

    let signal_result = shutdown_signal().await;
    if signal_result.is_ok() {
        info!("shutdown signal received");
    }
    let stop_result = node.stop().await.context("stop node");

    signal_result.context("wait for shutdown signal")?;
    stop_result
}

fn init_tracing(cli: &Cli) -> Result<TracingGuard> {
    runtime_core::init_tracing(cli.verbosity(), file_sink(cli)?).context("initialize tracing")
}

fn file_sink(cli: &Cli) -> Result<Option<FileSink<'_>>> {
    let Some(dir) = cli.log_dir_path.as_deref() else {
        if cli.log_dir_prefix.is_some() {
            bail!("--log.dir.prefix requires --log.dir.path");
        }
        return Ok(None);
    };

    Ok(Some(FileSink {
        dir,
        prefix: cli.log_dir_prefix.as_deref().unwrap_or(DEFAULT_LOG_PREFIX),
    }))
}

fn build_devnet_config(cli: &Cli) -> Result<node::Config> {
    let listen_address = selected_listen_address(cli)?;
    let chain_config = genesis::load_chain_config(cli.genesis_config.as_deref())?;
    let validators_path = workspace_path(DEFAULT_VALIDATORS_PATH);
    let genesis_state = genesis::load_or_synthesize_state(
        cli.genesis_state.as_deref(),
        &chain_config,
        &validators_path,
    )?;
    let genesis_block = genesis::anchor_block_for_state(&genesis_state)?;

    let p2p = HostOptions::try_new(
        listen_address,
        AGENT_VERSION,
        Path::new(DEFAULT_IDENTITY_PATH),
        cli.devnet_bootnodes.as_deref(),
    )
    .context("build p2p host options")?;

    let duties = runtime_duties::Config::default()
        .with_validators_path(validators_path)
        .context("build duties config")?
        .with_genesis_time_unix(runtime_duties::GenesisTimeUnix::new(
            genesis_state.config.genesis_time,
        ));

    Ok(node::Config {
        node: NodeConfig::default(),
        p2p,
        duties,
        http_addr: parse_socket_addr(DEFAULT_HTTP_ADDR)?,
        metrics_addr: parse_socket_addr(DEFAULT_METRICS_ADDR)?,
        genesis_state,
        genesis_block,
    })
}

fn selected_listen_address(cli: &Cli) -> Result<&str> {
    let listen_address = cli
        .listen_address()
        .context("--devnet-listen-addresses must include at least one address")?;
    if cli.devnet_listen_addresses.len() > 1 {
        warn!(
            configured = cli.devnet_listen_addresses.len(),
            selected = listen_address,
            "runtime currently supports a single devnet listen address; using the first"
        );
    }
    Ok(listen_address)
}

fn parse_socket_addr(raw: &str) -> Result<SocketAddr> {
    raw.parse()
        .with_context(|| format!("parse socket address {raw:?}"))
}

fn workspace_path(relative: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|crates| crates.parent()) {
        Some(root) => root.join(relative),
        None => manifest_dir.join(relative),
    }
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("listen for SIGINT")?,
        _ = sigterm.recv() => {},
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<()> {
    tokio::signal::ctrl_c().await.context("listen for SIGINT")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use ssz::HashTreeRoot;

    #[test]
    fn workspace_path_resolves_repo_file() {
        assert!(workspace_path("Cargo.toml").exists());
    }

    #[test]
    fn build_devnet_config_synthesizes_genesis_when_state_is_absent() {
        let cli = Cli::try_parse_from(["lean-beacon"]).expect("parse defaults");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(config.genesis_state.config.num_validators, 30);
        assert_eq!(
            config.genesis_block.state_root,
            config.genesis_state.hash_tree_root().into()
        );
    }
}
