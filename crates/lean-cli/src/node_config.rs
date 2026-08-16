//! Builds the devnet [`node::Config`] from parsed CLI flags.
//!
//! This is the CLI-to-node wiring layer: address defaults, identity-path
//! precedence, storage-backend mapping, genesis synthesis, and
//! validator-group selection.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use runtime::core::NodeConfig;
use runtime::p2p::HostOptions;
use tracing::warn;

use crate::cli::{Cli, StorageBackend};
use crate::genesis;

/// libp2p agent string advertised to peers.
///
/// `env!` resolves against this crate, matching `cli.rs`'s
/// `#[command(version)]`, which already reports lean-cli's version.
/// `bin/lean-rust/tests/agent_version.rs` pins the two together.
pub const AGENT_VERSION: &str = concat!("lean-rust/", env!("CARGO_PKG_VERSION"));

const DEFAULT_HTTP_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5052);
const DEFAULT_IDENTITY_PATH: &str = "p2p_priv_key";
const DEFAULT_METRICS_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);

// Not the same default as `runtime::duties::DEFAULT_VALIDATORS_PATH`, and this
// one wins: `build_devnet_config` always overrides the field, so the `runtime`
// default is only ever seen by callers that build a `duties::Config`
// themselves. Kept separate because that one resolves relative to `runtime`'s
// own manifest dir, while this one is made absolute by `workspace_path`.
const DEFAULT_VALIDATORS_PATH: &str = "crates/runtime/tests/duties_fixtures/validators.yaml";

/// Assembles the devnet node configuration from the parsed CLI.
///
/// # Errors
///
/// Returns an error if no listen address was configured, if genesis cannot
/// be loaded or synthesized, if the libp2p host options are invalid, if the
/// duties inputs are rejected, or if `--storage persistent` was given
/// without `--storage-path`.
pub fn build_devnet_config(cli: &Cli) -> Result<node::Config> {
    let listen_address = listen_address(cli)?;
    let chain_config = genesis::load_chain_config(cli.genesis_config.as_deref())?;
    let validators_path = validators_path(cli);
    let genesis_state = genesis::load_or_synthesize_state(
        cli.genesis_state.as_deref(),
        &chain_config,
        &validators_path,
    )?;
    let genesis_block = genesis::anchor_block_for_state(&genesis_state)?;
    let identity_path = identity_path(cli);

    let p2p = HostOptions::try_new(
        listen_address,
        AGENT_VERSION,
        &identity_path,
        cli.devnet_bootnodes.as_deref(),
    )
    .context("build p2p host options")?;

    // `--node-id` is the only override: leaving it unset keeps the group that
    // `Config::default` already carries, rather than reading that default back
    // out and feeding it through the validating builder again.
    let mut duties = runtime::duties::Config::default()
        .with_validators_path(validators_path)
        .context("set duties validators path")?;
    if let Some(group) = cli.node_id.as_deref() {
        duties = duties
            .with_validator_group(group)
            .context("set duties validator group")?;
    }
    let duties = duties.with_genesis_time_unix(runtime::duties::GenesisTimeUnix::new(
        genesis_state.config.genesis_time,
    ));

    // `--metrics` is accepted for local-pq CLI compatibility. Metrics are
    // already always wired into the current devnet node composition.
    Ok(node::Config {
        node: NodeConfig::default(),
        p2p,
        duties,
        http_addr: socket_addr(cli.http_address, cli.http_port, DEFAULT_HTTP_ADDR),
        metrics_addr: socket_addr(cli.metrics_address, cli.metrics_port, DEFAULT_METRICS_ADDR),
        genesis_state,
        genesis_block,
        storage: storage_kind(cli)?,
        validator_secrets_dir: cli.validator_secrets_dir.clone(),
    })
}

fn listen_address(cli: &Cli) -> Result<&str> {
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

fn socket_addr(address: Option<IpAddr>, port: Option<u16>, default: SocketAddr) -> SocketAddr {
    SocketAddr::new(
        address.unwrap_or_else(|| default.ip()),
        port.unwrap_or(default.port()),
    )
}

fn storage_kind(cli: &Cli) -> Result<node::StorageKind> {
    // A `--storage-path` given under the memory backend is ignored, with a
    // warning from `startup::warn_unwired_flags`.
    match cli.storage {
        StorageBackend::Memory => Ok(node::StorageKind::Memory),
        StorageBackend::Persistent => Ok(node::StorageKind::Persistent(
            cli.storage_path
                .clone()
                .context("--storage persistent requires --storage-path")?,
        )),
    }
}

fn validators_path(cli: &Cli) -> PathBuf {
    cli.validator_registry_path
        .clone()
        .unwrap_or_else(|| workspace_path(DEFAULT_VALIDATORS_PATH))
}

fn identity_path(cli: &Cli) -> PathBuf {
    if let Some(path) = &cli.private_key_path {
        path.clone()
    } else if let Some(data_dir) = &cli.data_dir {
        data_dir.join(DEFAULT_IDENTITY_PATH)
    } else {
        PathBuf::from(DEFAULT_IDENTITY_PATH)
    }
}

// Pops exactly two ancestors of the compiling crate's manifest directory.
// That reaches the repository root from `crates/lean-cli` (-> `crates` ->
// root) and, by coincidence of equal nesting depth, also reached it from
// the previous home at `bin/lean-rust` (-> `bin` -> root). The
// `workspace_path_resolves_repo_file` test below is the guard on that
// assumption: move this function to any other nesting depth and it breaks.
fn workspace_path(relative: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|crates| crates.parent()) {
        Some(root) => root.join(relative),
        None => manifest_dir.join(relative),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;
    use ssz::HashTreeRoot;

    fn write_validator_registry(dir: &Path) -> PathBuf {
        let path = dir.join("validators.yaml");
        std::fs::write(&path, "ream_0:\n  - 0\nleanrust_1:\n  - 1\n")
            .expect("write validator registry");
        // The genesis pubkey manifest is REQUIRED: State.validators is the sole
        // source of the validator count, so synthesis refuses an assignment file
        // with no sibling manifest.
        let pk0 = "00".repeat(52);
        let pk1 = "01".repeat(52);
        std::fs::write(
            dir.join("genesis_validators.yaml"),
            format!("genesis_validators:\n  - {pk0}\n  - {pk1}\n"),
        )
        .expect("write genesis validator manifest");
        path
    }

    fn as_str(path: &Path) -> &str {
        path.to_str().expect("test path must be utf-8")
    }

    /// Parses `args` and builds the config, for the cases that expect both to
    /// succeed. Tests asserting a failure call the two steps directly.
    fn config_from<const N: usize>(args: [&str; N]) -> node::Config {
        let cli = Cli::try_parse_from(args).expect("parse CLI args");
        build_devnet_config(&cli).expect("build config")
    }

    #[test]
    fn workspace_path_resolves_repo_file() {
        assert!(workspace_path("Cargo.toml").exists());
    }

    #[test]
    fn build_devnet_config_synthesizes_genesis_when_state_is_absent() {
        let config = config_from(["lean-rust"]);

        assert_eq!(config.genesis_state.num_validators(), 30);
        assert_eq!(
            config.genesis_block.state_root,
            config.genesis_state.hash_tree_root().into()
        );
    }

    #[test]
    fn build_devnet_config_uses_validator_registry_and_node_id() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let validators_path = write_validator_registry(dir.path());
        let config = config_from([
            "lean-rust",
            "--validator-registry-path",
            as_str(&validators_path),
            "--node-id",
            "leanrust_1",
        ]);

        assert_eq!(config.duties.validators_path(), validators_path.as_path());
        assert_eq!(config.duties.validator_group(), "leanrust_1");
        assert_eq!(config.genesis_state.num_validators(), 2);
    }

    #[test]
    fn build_devnet_config_uses_data_dir_for_default_identity_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = dir.path().join("node-data");
        let config = config_from(["lean-rust", "--data-dir", as_str(&data_dir)]);

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
        let config = config_from([
            "lean-rust",
            "--data-dir",
            as_str(&data_dir),
            "--private-key-path",
            as_str(&private_key_path),
        ]);

        assert_eq!(
            config.p2p.identity_path().as_path(),
            private_key_path.as_path()
        );
    }

    #[test]
    fn build_devnet_config_wires_http_and_metrics_addresses() {
        let config = config_from([
            "lean-rust",
            "--http-address",
            "0.0.0.0",
            "--http-port",
            "5053",
            "--metrics",
            "--metrics-address",
            "127.0.0.1",
            "--metrics-port",
            "8081",
        ]);

        assert_eq!(config.http_addr, "0.0.0.0:5053".parse().expect("addr"));
        assert_eq!(config.metrics_addr, "127.0.0.1:8081".parse().expect("addr"));
    }

    #[test]
    fn build_devnet_config_falls_back_to_the_default_addresses() {
        let config = config_from(["lean-rust"]);

        assert_eq!(config.http_addr, DEFAULT_HTTP_ADDR);
        assert_eq!(config.metrics_addr, DEFAULT_METRICS_ADDR);
    }

    #[test]
    fn build_devnet_config_metrics_flag_is_compatibility_noop() {
        let without_metrics = config_from(["lean-rust"]);
        let with_metrics = config_from(["lean-rust", "--metrics"]);

        assert_eq!(without_metrics.metrics_addr, with_metrics.metrics_addr);
    }

    #[test]
    fn build_devnet_config_defaults_to_memory_storage() {
        let config = config_from(["lean-rust"]);
        assert!(matches!(config.storage, node::StorageKind::Memory));
    }

    #[test]
    fn memory_backend_ignores_storage_path() {
        // --storage-path under the memory backend is ignored (with a startup
        // warning); the resolved backend stays Memory.
        let config = config_from(["lean-rust", "--storage-path", "/tmp/ignored"]);
        assert!(matches!(config.storage, node::StorageKind::Memory));
    }

    #[test]
    fn persistent_storage_without_path_is_rejected() {
        let cli = Cli::try_parse_from(["lean-rust", "--storage", "persistent"])
            .expect("parse persistent without path");
        let err = build_devnet_config(&cli).expect_err("missing storage path must fail");
        assert!(err.to_string().contains("--storage-path"), "got {err}");
    }

    #[test]
    fn build_devnet_config_maps_persistent_storage_path() {
        let config = config_from([
            "lean-rust",
            "--storage",
            "persistent",
            "--storage-path",
            "/tmp/lean-store",
        ]);
        assert!(matches!(
            config.storage,
            node::StorageKind::Persistent(ref p) if p == Path::new("/tmp/lean-store")
        ));
    }
}
