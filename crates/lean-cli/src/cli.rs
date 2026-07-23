//! Command-line interface for `lean-rust`.

use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use runtime::observability::Verbosity;

/// Default libp2p QUIC-v1 listen address for devnet nodes.
pub const DEFAULT_DEVNET_LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/udp/9000/quic-v1";

/// Persistence backend selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StorageBackend {
    /// In-memory, non-durable (default).
    Memory,
    /// Durable on-disk store (requires `--storage-path`).
    Persistent,
}

/// Parsed `lean-rust` CLI.
#[derive(Debug, Parser)]
#[command(name = "lean-rust", version, about = "Lean Ethereum devnet0 client")]
pub struct Cli {
    /// Root directory for node data.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// libp2p listen multiaddrs. The current runtime supports one address; the first value is used.
    #[arg(long, value_delimiter = ',', default_value = DEFAULT_DEVNET_LISTEN_ADDRESS)]
    pub devnet_listen_addresses: Vec<String>,

    /// Path to bootnodes YAML.
    #[arg(long, visible_alias = "bootnodes")]
    pub devnet_bootnodes: Option<PathBuf>,

    /// Path to devnet chain-config YAML.
    #[arg(long, visible_alias = "network")]
    pub genesis_config: Option<PathBuf>,

    /// Path to SSZ-encoded genesis state.
    #[arg(long)]
    pub genesis_state: Option<PathBuf>,

    /// Path to local-pq validator registry YAML.
    #[arg(long)]
    pub validator_registry_path: Option<PathBuf>,

    /// Directory of per-validator secret key records (`validator_<i>.ssz`,
    /// `OtsKeyState` SSZ) produced by the offline keygen. Required only when this
    /// node runs local validators; the node loads ONLY its own validators' secrets
    /// from here to sign attestations and blocks. An observer (no local
    /// validators) starts without it.
    #[arg(long, value_name = "DIR")]
    pub validator_secrets_dir: Option<PathBuf>,

    /// Local validator-group/node identifier from the validator registry.
    #[arg(long)]
    pub node_id: Option<String>,

    /// Path to the libp2p private key for this node.
    #[arg(long)]
    pub private_key_path: Option<PathBuf>,

    /// HTTP listen address.
    #[arg(long)]
    pub http_address: Option<IpAddr>,

    /// HTTP listen port.
    #[arg(long)]
    pub http_port: Option<u16>,

    /// Accepted for CLI-compat with the local-pq devnet runner; the value is
    /// NOT applied to the HTTP server (no CORS layer is wired). Setting this
    /// produces a startup warning. Bare `--http-allow-origin` defaults to `*`.
    #[arg(long, num_args = 0..=1, default_missing_value = "*")]
    pub http_allow_origin: Option<String>,

    /// Enable the metrics endpoint.
    #[arg(long)]
    pub metrics: bool,

    /// Metrics listen address.
    #[arg(long)]
    pub metrics_address: Option<IpAddr>,

    /// Metrics listen port.
    #[arg(long)]
    pub metrics_port: Option<u16>,

    /// Persistence backend: in-memory (default) or a durable on-disk store.
    #[arg(long, value_enum, default_value_t = StorageBackend::Memory)]
    pub storage: StorageBackend,

    /// Filesystem path for the persistent store (required when
    /// `--storage persistent`).
    #[arg(long)]
    pub storage_path: Option<PathBuf>,

    /// Log level used when `RUST_LOG` is not set.
    #[arg(long = "log-level", default_value_t = Verbosity::Info)]
    pub log_level: Verbosity,

    /// Enable trace-level logging when `RUST_LOG` is not set.
    #[arg(long)]
    pub debug: bool,

    /// Directory for optional file logs.
    #[arg(long = "log.dir.path")]
    pub log_dir_path: Option<PathBuf>,

    /// Filename prefix for optional file logs.
    #[arg(long = "log.dir.prefix")]
    pub log_dir_prefix: Option<String>,

    /// Optional command to run instead of starting the node.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// `lean-rust` subcommands.
#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Print the canonical lean-rust devnet0 chain config YAML.
    DevnetConfig,
    /// Generate a libp2p node private key and write it to disk.
    GeneratePrivateKey {
        /// Output path for protobuf-encoded libp2p key bytes.
        #[arg(long)]
        output_path: PathBuf,
    },
    /// Print the libp2p peer ID for an existing private key file.
    PeerId {
        /// Path to protobuf or local-pq raw secp256k1 key bytes.
        #[arg(long)]
        private_key_path: PathBuf,
    },
    /// Offline: pre-generate per-validator XMSS attestation keys and the
    /// coordinator-canonical `genesis_validators` pubkey manifest.
    GenerateValidatorKeys {
        /// Number of validator keys to generate (indices `0..count`).
        #[arg(long)]
        count: u64,
        /// Directory for the per-validator `validator_<i>.ssz` secret files.
        #[arg(long)]
        out_dir: PathBuf,
        /// Output path for the `genesis_validators` manifest.
        #[arg(long)]
        manifest_path: PathBuf,
        /// Activation epoch; must be a multiple of the sqrt-lifetime boundary
        /// (2^16 = 65536) or it is rejected (never silently rounded). Default 0.
        #[arg(long)]
        activation_epoch: Option<u64>,
    },
}

impl Cli {
    /// Returns the listen address selected for the current single-listener runtime.
    #[must_use]
    pub fn listen_address(&self) -> Option<&str> {
        self.devnet_listen_addresses.first().map(String::as_str)
    }

    /// Returns the effective verbosity before `RUST_LOG` override handling.
    #[must_use]
    pub const fn verbosity(&self) -> Verbosity {
        if self.debug {
            Verbosity::Trace
        } else {
            self.log_level
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn loopback() -> IpAddr {
        IpAddr::from([127, 0, 0, 1])
    }

    fn unspecified() -> IpAddr {
        IpAddr::from([0, 0, 0, 0])
    }

    #[test]
    fn parses_generate_private_key_subcommand() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "generate-private-key",
            "--output-path",
            "/tmp/key.pb",
        ])
        .expect("parse keygen subcommand");

        assert_eq!(
            cli.command,
            Some(Command::GeneratePrivateKey {
                output_path: PathBuf::from("/tmp/key.pb")
            })
        );
    }

    #[test]
    fn parses_generate_validator_keys_subcommand() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "generate-validator-keys",
            "--count",
            "2",
            "--out-dir",
            "/tmp/secrets",
            "--manifest-path",
            "/tmp/genesis_validators.yaml",
            "--activation-epoch",
            "65536",
        ])
        .expect("parse generate-validator-keys subcommand");

        assert_eq!(
            cli.command,
            Some(Command::GenerateValidatorKeys {
                count: 2,
                out_dir: PathBuf::from("/tmp/secrets"),
                manifest_path: PathBuf::from("/tmp/genesis_validators.yaml"),
                activation_epoch: Some(1 << 16), // aligned sqrt-lifetime boundary
            })
        );
    }

    #[test]
    fn parses_devnet_config_subcommand() {
        let cli = Cli::try_parse_from(["lean-rust", "devnet-config"])
            .expect("parse devnet-config subcommand");

        assert_eq!(cli.command, Some(Command::DevnetConfig));
    }

    #[test]
    fn parses_peer_id_subcommand() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "peer-id",
            "--private-key-path",
            "/tmp/node.key",
        ])
        .expect("parse peer-id subcommand");

        assert_eq!(
            cli.command,
            Some(Command::PeerId {
                private_key_path: PathBuf::from("/tmp/node.key")
            })
        );
    }

    #[test]
    fn parses_root_flags() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "--data-dir",
            "data",
            "--devnet-listen-addresses",
            "/ip4/127.0.0.1/udp/9001/quic-v1",
            "--devnet-bootnodes",
            "nodes.yaml",
            "--genesis-config",
            "devnet.yaml",
            "--genesis-state",
            "genesis.ssz",
            "--validator-registry-path",
            "validators.yaml",
            "--node-id",
            "leanrust_1",
            "--private-key-path",
            "node1.key",
            "--http-address",
            "127.0.0.1",
            "--http-port",
            "5052",
            "--http-allow-origin",
            "http://localhost:3000",
            "--metrics",
            "--metrics-address",
            "127.0.0.1",
            "--metrics-port",
            "8080",
            "--log-level",
            "debug",
            "--log.dir.path",
            "logs",
            "--log.dir.prefix",
            "lean",
        ])
        .expect("parse root flags");

        assert_eq!(
            cli.listen_address(),
            Some("/ip4/127.0.0.1/udp/9001/quic-v1")
        );
        assert_eq!(cli.devnet_bootnodes, Some(PathBuf::from("nodes.yaml")));
        assert_eq!(cli.genesis_config, Some(PathBuf::from("devnet.yaml")));
        assert_eq!(cli.genesis_state, Some(PathBuf::from("genesis.ssz")));
        assert_eq!(cli.data_dir, Some(PathBuf::from("data")));
        assert_eq!(
            cli.validator_registry_path,
            Some(PathBuf::from("validators.yaml"))
        );
        assert_eq!(cli.node_id.as_deref(), Some("leanrust_1"));
        assert_eq!(cli.private_key_path, Some(PathBuf::from("node1.key")));
        assert_eq!(cli.http_address, Some(loopback()));
        assert_eq!(cli.http_port, Some(5052));
        assert_eq!(
            cli.http_allow_origin.as_deref(),
            Some("http://localhost:3000")
        );
        assert!(cli.metrics);
        assert_eq!(cli.metrics_address, Some(loopback()));
        assert_eq!(cli.metrics_port, Some(8080));
        assert_eq!(cli.verbosity(), Verbosity::Debug);
        assert_eq!(cli.log_dir_path, Some(PathBuf::from("logs")));
        assert_eq!(cli.log_dir_prefix.as_deref(), Some("lean"));
    }

    #[test]
    fn parses_network_alias_for_genesis_config() {
        let cli = Cli::try_parse_from(["lean-rust", "--network", "config.yaml"])
            .expect("parse network alias");

        assert_eq!(cli.genesis_config, Some(PathBuf::from("config.yaml")));
    }

    #[test]
    fn parses_bootnodes_alias_for_devnet_bootnodes() {
        let cli = Cli::try_parse_from(["lean-rust", "--bootnodes", "bootnodes.yaml"])
            .expect("parse bootnodes alias");

        assert_eq!(cli.devnet_bootnodes, Some(PathBuf::from("bootnodes.yaml")));
    }

    #[test]
    fn parses_bare_http_allow_origin_as_wildcard() {
        let cli = Cli::try_parse_from(["lean-rust", "--http-allow-origin"])
            .expect("parse bare http allow origin");

        assert_eq!(cli.http_allow_origin.as_deref(), Some("*"));
    }

    #[test]
    fn parses_planned_local_pq_compose_command() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "--data-dir=/data",
            "--network=/genesis/config.yaml",
            "--genesis-state=/genesis/genesis.ssz",
            "--validator-registry-path=/genesis/validators.yaml",
            "--node-id=leanrust_1",
            "--private-key-path=/config/keys/node1.key",
            "--bootnodes=/genesis/bootnodes.rust.yaml",
            "--devnet-listen-addresses=/ip4/0.0.0.0/udp/9000/quic-v1",
            "--http-address=0.0.0.0",
            "--http-port=5052",
            "--http-allow-origin=*",
            "--metrics",
            "--metrics-address=0.0.0.0",
            "--metrics-port=8080",
            "--log.dir.path=/var/lean-logs",
        ])
        .expect("parse planned lean-rust compose command");

        assert_eq!(cli.data_dir, Some(PathBuf::from("/data")));
        assert_eq!(
            cli.genesis_config,
            Some(PathBuf::from("/genesis/config.yaml"))
        );
        assert_eq!(
            cli.genesis_state,
            Some(PathBuf::from("/genesis/genesis.ssz"))
        );
        assert_eq!(
            cli.validator_registry_path,
            Some(PathBuf::from("/genesis/validators.yaml"))
        );
        assert_eq!(cli.node_id.as_deref(), Some("leanrust_1"));
        assert_eq!(
            cli.private_key_path,
            Some(PathBuf::from("/config/keys/node1.key"))
        );
        assert_eq!(
            cli.devnet_bootnodes,
            Some(PathBuf::from("/genesis/bootnodes.rust.yaml"))
        );
        assert_eq!(cli.listen_address(), Some("/ip4/0.0.0.0/udp/9000/quic-v1"));
        assert_eq!(cli.http_address, Some(unspecified()));
        assert_eq!(cli.http_port, Some(5052));
        assert_eq!(cli.http_allow_origin.as_deref(), Some("*"));
        assert!(cli.metrics);
        assert_eq!(cli.metrics_address, Some(unspecified()));
        assert_eq!(cli.metrics_port, Some(8080));
        assert_eq!(cli.verbosity(), Verbosity::Info);
        assert_eq!(cli.log_dir_path, Some(PathBuf::from("/var/lean-logs")));
    }

    #[test]
    fn debug_forces_trace_verbosity() {
        let cli = Cli::try_parse_from(["lean-rust", "--log-level", "error", "--debug"])
            .expect("parse debug flag");

        assert_eq!(cli.verbosity(), Verbosity::Trace);
    }

    #[test]
    fn parses_persistent_storage_flags() {
        let cli = Cli::try_parse_from([
            "lean-rust",
            "--storage",
            "persistent",
            "--storage-path",
            "/data/db",
        ])
        .expect("parse storage flags");

        assert_eq!(cli.storage, StorageBackend::Persistent);
        assert_eq!(cli.storage_path, Some(PathBuf::from("/data/db")));
    }

    #[test]
    fn storage_defaults_to_memory() {
        let cli = Cli::try_parse_from(["lean-rust"]).expect("parse defaults");
        assert_eq!(cli.storage, StorageBackend::Memory);
        assert_eq!(cli.storage_path, None);
    }
}
