//! Command-line interface for `lean-beacon`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use runtime_core::Verbosity;

/// Default libp2p QUIC-v1 listen address for devnet nodes.
pub const DEFAULT_DEVNET_LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/udp/9000/quic-v1";

/// Parsed `lean-beacon` CLI.
#[derive(Debug, Parser)]
#[command(name = "lean-beacon", version, about = "Lean Ethereum devnet0 client")]
pub struct Cli {
    /// libp2p listen multiaddrs. The current runtime supports one address; the first value is used.
    #[arg(long, value_delimiter = ',', default_value = DEFAULT_DEVNET_LISTEN_ADDRESS)]
    pub devnet_listen_addresses: Vec<String>,

    /// Path to bootnodes YAML.
    #[arg(long)]
    pub devnet_bootnodes: Option<PathBuf>,

    /// Path to devnet chain-config YAML.
    #[arg(long)]
    pub genesis_config: Option<PathBuf>,

    /// Path to SSZ-encoded genesis state.
    #[arg(long)]
    pub genesis_state: Option<PathBuf>,

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
            "--devnet-listen-addresses",
            "/ip4/127.0.0.1/udp/9001/quic-v1",
            "--devnet-bootnodes",
            "nodes.yaml",
            "--genesis-config",
            "devnet.yaml",
            "--genesis-state",
            "genesis.ssz",
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
        assert_eq!(cli.verbosity(), Verbosity::Debug);
        assert_eq!(cli.log_dir_path, Some(PathBuf::from("logs")));
        assert_eq!(cli.log_dir_prefix.as_deref(), Some("lean"));
    }

    #[test]
    fn debug_forces_trace_verbosity() {
        let cli = Cli::try_parse_from(["lean-beacon", "--log-level", "error", "--debug"])
            .expect("parse debug flag");

        assert_eq!(cli.verbosity(), Verbosity::Trace);
    }
}
