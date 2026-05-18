//! Command-line interface for `lean-beacon`.

use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use runtime_core::Verbosity;

/// Default libp2p QUIC-v1 listen address for devnet nodes.
pub const DEFAULT_DEVNET_LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/udp/9000/quic-v1";

/// Parsed `lean-beacon` CLI.
#[derive(Debug, Parser)]
#[command(name = "lean-beacon", version, about = "Lean Ethereum devnet0 client")]
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

    /// Allowed HTTP origin. Bare `--http-allow-origin` defaults to `*`.
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

/// `lean-beacon` subcommands.
#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Generate a libp2p node private key and write it to disk.
    GeneratePrivateKey {
        /// Output path for protobuf-encoded libp2p key bytes.
        #[arg(long)]
        output_path: PathBuf,
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
            "lean-beacon",
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
    fn parses_root_flags() {
        let cli = Cli::try_parse_from([
            "lean-beacon",
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
        let cli = Cli::try_parse_from(["lean-beacon", "--network", "config.yaml"])
            .expect("parse network alias");

        assert_eq!(cli.genesis_config, Some(PathBuf::from("config.yaml")));
    }

    #[test]
    fn parses_bootnodes_alias_for_devnet_bootnodes() {
        let cli = Cli::try_parse_from(["lean-beacon", "--bootnodes", "bootnodes.yaml"])
            .expect("parse bootnodes alias");

        assert_eq!(cli.devnet_bootnodes, Some(PathBuf::from("bootnodes.yaml")));
    }

    #[test]
    fn parses_bare_http_allow_origin_as_wildcard() {
        let cli = Cli::try_parse_from(["lean-beacon", "--http-allow-origin"])
            .expect("parse bare http allow origin");

        assert_eq!(cli.http_allow_origin.as_deref(), Some("*"));
    }

    #[test]
    fn parses_planned_local_pq_compose_command() {
        let cli = Cli::try_parse_from([
            "lean-beacon",
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
            "--debug",
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
        assert_eq!(cli.verbosity(), Verbosity::Trace);
        assert_eq!(cli.log_dir_path, Some(PathBuf::from("/var/lean-logs")));
    }

    #[test]
    fn debug_forces_trace_verbosity() {
        let cli = Cli::try_parse_from(["lean-beacon", "--log-level", "error", "--debug"])
            .expect("parse debug flag");

        assert_eq!(cli.verbosity(), Verbosity::Trace);
    }
}
