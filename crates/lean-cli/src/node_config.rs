//! Builds the devnet [`node::Config`] from parsed CLI flags.
//!
//! This is the CLI-to-node wiring layer: address defaults, identity-path
//! precedence, storage-backend mapping, genesis synthesis, and
//! validator-group selection.

use std::net::{IpAddr, SocketAddr};
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

const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:5052";
const DEFAULT_IDENTITY_PATH: &str = "p2p_priv_key";
const DEFAULT_METRICS_ADDR: &str = "127.0.0.1:9090";
const DEFAULT_VALIDATORS_PATH: &str = "crates/runtime/tests/duties_fixtures/validators.yaml";

/// Assembles the devnet node configuration from the parsed CLI.
///
/// # Errors
///
/// Returns an error if no listen address was configured, if genesis cannot
/// be loaded or synthesized, if the libp2p host options are invalid, if a
/// socket address fails to parse, or if `--storage persistent` was given
/// without `--storage-path`.
pub fn build_devnet_config(cli: &Cli) -> Result<node::Config> {
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

    let duties = runtime::duties::Config::default()
        .with_validators_path(validators_path)
        .context("build duties config")?
        .with_validator_group(selected_validator_group(cli))
        .context("build duties config")?
        .with_genesis_time_unix(runtime::duties::GenesisTimeUnix::new(
            genesis_state.config.genesis_time,
        ));

    let storage = match cli.storage {
        StorageBackend::Memory => {
            if let Some(path) = cli.storage_path.as_deref() {
                warn!(
                    path = %path.display(),
                    "--storage-path is ignored because --storage is memory; pass --storage persistent to use it",
                );
            }
            node::StorageKind::Memory
        }
        StorageBackend::Persistent => {
            let path = cli
                .storage_path
                .clone()
                .context("--storage persistent requires --storage-path")?;
            node::StorageKind::Persistent(path)
        }
    };

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
        storage,
        validator_secrets_dir: cli.validator_secrets_dir.clone(),
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
    cli.node_id.clone().unwrap_or_else(|| {
        runtime::duties::Config::default()
            .validator_group()
            .to_owned()
    })
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

    fn parse_path(path: &Path) -> &str {
        path.to_str().expect("test path must be utf-8")
    }

    #[test]
    fn workspace_path_resolves_repo_file() {
        assert!(workspace_path("Cargo.toml").exists());
    }

    #[test]
    fn build_devnet_config_synthesizes_genesis_when_state_is_absent() {
        let cli = Cli::try_parse_from(["lean-rust"]).expect("parse defaults");

        let config = build_devnet_config(&cli).expect("build config");

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
        let cli = Cli::try_parse_from([
            "lean-rust",
            "--validator-registry-path",
            parse_path(&validators_path),
            "--node-id",
            "leanrust_1",
        ])
        .expect("parse local-pq duties flags");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(config.duties.validators_path(), validators_path.as_path());
        assert_eq!(config.duties.validator_group(), "leanrust_1");
        assert_eq!(config.genesis_state.num_validators(), 2);
    }

    #[test]
    fn build_devnet_config_uses_data_dir_for_default_identity_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = dir.path().join("node-data");
        let cli = Cli::try_parse_from(["lean-rust", "--data-dir", parse_path(&data_dir)])
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
            "lean-rust",
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
        ])
        .expect("parse api flags");

        let config = build_devnet_config(&cli).expect("build config");

        assert_eq!(config.http_addr, "0.0.0.0:5053".parse().expect("addr"));
        assert_eq!(config.metrics_addr, "127.0.0.1:8081".parse().expect("addr"));
    }

    #[test]
    fn build_devnet_config_metrics_flag_is_compatibility_noop() {
        let without_metrics =
            Cli::try_parse_from(["lean-rust"]).expect("parse without metrics flag");
        let with_metrics =
            Cli::try_parse_from(["lean-rust", "--metrics"]).expect("parse with metrics flag");

        let without_metrics =
            build_devnet_config(&without_metrics).expect("build config without metrics flag");
        let with_metrics =
            build_devnet_config(&with_metrics).expect("build config with metrics flag");

        assert_eq!(without_metrics.metrics_addr, with_metrics.metrics_addr);
    }

    #[test]
    fn build_devnet_config_defaults_to_memory_storage() {
        let cli = Cli::try_parse_from(["lean-rust"]).expect("parse defaults");
        let config = build_devnet_config(&cli).expect("build config");
        assert!(matches!(config.storage, node::StorageKind::Memory));
    }

    #[test]
    fn memory_backend_ignores_storage_path() {
        // --storage-path under the memory backend is ignored (with a startup
        // warning); the resolved backend stays Memory.
        let cli = Cli::try_parse_from(["lean-rust", "--storage-path", "/tmp/ignored"])
            .expect("parse memory with stray storage path");
        let config = build_devnet_config(&cli).expect("build config");
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
        let cli = Cli::try_parse_from([
            "lean-rust",
            "--storage",
            "persistent",
            "--storage-path",
            "/tmp/lean-store",
        ])
        .expect("parse persistent storage flags");
        let config = build_devnet_config(&cli).expect("build config");
        assert!(matches!(
            config.storage,
            node::StorageKind::Persistent(ref p) if p == Path::new("/tmp/lean-store")
        ));
    }
}
