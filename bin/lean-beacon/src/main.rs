//! `lean-beacon` binary entry point.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use lean_core::NodeConfig;
use lean_observability::{FileSink, TracingGuard};
use lean_p2p_host::HostOptions;
use tracing::{info, warn};

use lean_cli::cli::{Cli, Command};
use lean_cli::{genesis, keygen};

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
    log_startup_config(&cli);
    warn_unwired_flags(&cli);

    match &cli.command {
        Some(Command::DevnetConfig) => {
            print!(
                "{}",
                config::DEVNET_CONFIG
                    .to_yaml()
                    .context("serialize devnet config")?
            );
            return Ok(());
        }
        Some(Command::GeneratePrivateKey { output_path }) => {
            let peer_id =
                keygen::generate_and_write(output_path).context("generate private key")?;
            info!(%peer_id, path = %output_path.display(), "generated libp2p private key");
            return Ok(());
        }
        Some(Command::PeerId { private_key_path }) => {
            let peer_id = keygen::peer_id_from_file(private_key_path).context("derive peer id")?;
            println!("{peer_id}");
            return Ok(());
        }
        None => {}
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

fn warn_unwired_flags(cli: &Cli) {
    if let Some(origin) = cli.http_allow_origin.as_deref() {
        warn!(
            value = origin,
            "--http-allow-origin is accepted for CLI compatibility but NOT applied: no CORS layer is wired into the HTTP server. The HTTP API will respond with default axum headers regardless of this value.",
        );
    }
}

fn init_tracing(cli: &Cli) -> Result<TracingGuard> {
    lean_observability::init_tracing(cli.verbosity(), file_sink(cli)?).context("initialize tracing")
}

fn log_startup_config(cli: &Cli) {
    let rust_log = std::env::var_os("RUST_LOG");
    let rust_log_present = rust_log.is_some();
    let rust_log_non_empty = rust_log.as_deref().is_some_and(|value| !value.is_empty());

    info!(
        effective_verbosity = %cli.verbosity(),
        rust_log_present,
        rust_log_non_empty,
        data_dir = ?cli.data_dir,
        genesis_config = ?cli.genesis_config,
        genesis_state = ?cli.genesis_state,
        validator_registry_path = ?cli.validator_registry_path,
        node_id = ?cli.node_id,
        private_key_path = ?cli.private_key_path,
        devnet_bootnodes = ?cli.devnet_bootnodes,
        devnet_listen_addresses = ?cli.devnet_listen_addresses,
        http_address = ?cli.http_address,
        http_port = ?cli.http_port,
        http_allow_origin = ?cli.http_allow_origin,
        metrics_enabled = cli.metrics,
        metrics_address = ?cli.metrics_address,
        metrics_port = ?cli.metrics_port,
        log_dir_path = ?cli.log_dir_path,
        log_dir_prefix = ?selected_log_prefix(cli),
        "startup configuration",
    );
}

fn selected_log_prefix(cli: &Cli) -> Option<&str> {
    cli.log_dir_path
        .as_ref()
        .map(|_| cli.log_dir_prefix.as_deref().unwrap_or(DEFAULT_LOG_PREFIX))
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
    let validators_path = selected_validators_path(cli);
    let genesis_state = genesis::load_or_synthesize_state(
        cli.genesis_state.as_deref(),
        &chain_config,
        &validators_path,
    )?;
    let genesis_block = genesis::anchor_block_for_state(&genesis_state)?;
    let identity_path = selected_identity_path(cli);

    let p2p = HostOptions::try_new(
        listen_address,
        AGENT_VERSION,
        &identity_path,
        cli.devnet_bootnodes.as_deref(),
    )
    .context("build p2p host options")?;

    let duties = lean_duties::Config::default()
        .with_validators_path(validators_path)
        .context("build duties config")?
        .with_validator_group(selected_validator_group(cli))
        .context("build duties config")?
        .with_genesis_time_unix(lean_duties::GenesisTimeUnix::new(
            genesis_state.config.genesis_time,
        ));

    // `--metrics` is accepted for local-pq CLI compatibility. Metrics are
    // already always wired into the current devnet node composition.
    Ok(node::Config {
        node: NodeConfig::default(),
        p2p,
        duties,
        http_addr: selected_socket_addr(cli.http_address, cli.http_port, DEFAULT_HTTP_ADDR)?,
        metrics_addr: selected_socket_addr(
            cli.metrics_address,
            cli.metrics_port,
            DEFAULT_METRICS_ADDR,
        )?,
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

fn selected_socket_addr(
    address: Option<IpAddr>,
    port: Option<u16>,
    default_raw: &str,
) -> Result<SocketAddr> {
    let default = parse_socket_addr(default_raw)?;
    Ok(SocketAddr::new(
        address.unwrap_or_else(|| default.ip()),
        port.unwrap_or(default.port()),
    ))
}

fn selected_validators_path(cli: &Cli) -> PathBuf {
    cli.validator_registry_path
        .clone()
        .unwrap_or_else(|| workspace_path(DEFAULT_VALIDATORS_PATH))
}

fn selected_validator_group(cli: &Cli) -> String {
    cli.node_id
        .clone()
        .unwrap_or_else(|| lean_duties::Config::default().validator_group().to_owned())
}

fn selected_identity_path(cli: &Cli) -> PathBuf {
    if let Some(path) = &cli.private_key_path {
        path.clone()
    } else if let Some(data_dir) = &cli.data_dir {
        data_dir.join(DEFAULT_IDENTITY_PATH)
    } else {
        PathBuf::from(DEFAULT_IDENTITY_PATH)
    }
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

    fn write_validator_registry(dir: &Path) -> PathBuf {
        let path = dir.join("validators.yaml");
        std::fs::write(&path, "ream_0:\n  - 0\nleanrust_1:\n  - 1\n")
            .expect("write validator registry");
        path
    }

    fn parse_path(path: &Path) -> &str {
        path.to_str().expect("test path must be utf-8")
    }

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

    #[test]
    fn build_devnet_config_uses_validator_registry_and_node_id() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let validators_path = write_validator_registry(dir.path());
        let cli = Cli::try_parse_from([
            "lean-beacon",
            "--validator-registry-path",
            parse_path(&validators_path),
            "--node-id",
            "leanrust_1",
        ])
        .expect("parse local-pq duties flags");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(config.duties.validators_path(), validators_path.as_path());
        assert_eq!(config.duties.validator_group(), "leanrust_1");
        assert_eq!(config.genesis_state.config.num_validators, 2);
    }

    #[test]
    fn build_devnet_config_uses_data_dir_for_default_identity_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = dir.path().join("node-data");
        let cli = Cli::try_parse_from(["lean-beacon", "--data-dir", parse_path(&data_dir)])
            .expect("parse data dir");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(
            config.p2p.identity_path().as_path(),
            data_dir.join(DEFAULT_IDENTITY_PATH)
        );
    }

    #[test]
    fn build_devnet_config_private_key_path_overrides_data_dir() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = dir.path().join("node-data");
        let private_key_path = dir.path().join("keys/node1.key");
        let cli = Cli::try_parse_from([
            "lean-beacon",
            "--data-dir",
            parse_path(&data_dir),
            "--private-key-path",
            parse_path(&private_key_path),
        ])
        .expect("parse identity flags");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(
            config.p2p.identity_path().as_path(),
            private_key_path.as_path()
        );
    }

    #[test]
    fn build_devnet_config_wires_http_and_metrics_addresses() {
        let cli = Cli::try_parse_from([
            "lean-beacon",
            "--http-address",
            "0.0.0.0",
            "--http-port",
            "5053",
            "--metrics",
            "--metrics-address",
            "127.0.0.1",
            "--metrics-port",
            "8081",
        ])
        .expect("parse api flags");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(config.http_addr, "0.0.0.0:5053".parse().expect("addr"));
        assert_eq!(config.metrics_addr, "127.0.0.1:8081".parse().expect("addr"));
    }

    #[test]
    fn build_devnet_config_metrics_flag_is_compatibility_noop() {
        let without_metrics =
            Cli::try_parse_from(["lean-beacon"]).expect("parse without metrics flag");
        let with_metrics =
            Cli::try_parse_from(["lean-beacon", "--metrics"]).expect("parse with metrics flag");

        let without_metrics =
            build_devnet_config(&without_metrics).expect("build config without metrics flag");
        let with_metrics =
            build_devnet_config(&with_metrics).expect("build config with metrics flag");

        assert_eq!(without_metrics.metrics_addr, with_metrics.metrics_addr);
    }

    #[test]
    fn file_sink_rejects_prefix_without_path() {
        let cli = Cli::try_parse_from(["lean-beacon", "--log.dir.prefix", "lean"])
            .expect("parse log prefix");

        let err = file_sink(&cli).expect_err("log prefix without log path must fail");

        assert!(
            err.to_string()
                .contains("--log.dir.prefix requires --log.dir.path"),
            "got {err}"
        );
    }
}
