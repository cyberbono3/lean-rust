//! Subcommand dispatch for the `lean-rust` binary.
//!
//! Every subcommand is a terminal operation: it runs, prints, and the
//! process exits. Absence of a subcommand is the signal to start the
//! devnet node instead.

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::{Cli, Command};
use crate::{keygen, validator_keygen};

/// Outcome of [`dispatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Dispatch {
    /// A subcommand ran to completion; the caller should exit successfully.
    Handled,
    /// No subcommand was given; the caller should start the devnet node.
    RunNode,
}

/// Runs `cli.command` if one was given.
///
/// # Errors
///
/// Returns an error if the devnet config cannot be serialized, if key
/// generation fails, or if a peer id cannot be derived from the given file.
pub fn dispatch(cli: &Cli) -> Result<Dispatch> {
    let Some(command) = &cli.command else {
        return Ok(Dispatch::RunNode);
    };

    match command {
        Command::DevnetConfig => {
            print!(
                "{}",
                config::DEVNET_CONFIG
                    .to_yaml()
                    .context("serialize devnet config")?
            );
        }
        Command::GeneratePrivateKey { output_path } => {
            let peer_id =
                keygen::generate_and_write(output_path).context("generate private key")?;
            info!(%peer_id, path = %output_path.display(), "generated libp2p private key");
        }
        Command::PeerId { private_key_path } => {
            let peer_id = keygen::peer_id_from_file(private_key_path).context("derive peer id")?;
            println!("{peer_id}");
        }
        Command::GenerateValidatorKeys {
            count,
            out_dir,
            manifest_path,
            activation_epoch,
        } => {
            let params = validator_keygen::KeygenParams {
                count: *count,
                out_dir: out_dir.clone(),
                manifest_path: manifest_path.clone(),
                activation_epoch: activation_epoch.unwrap_or(0),
            };
            // ThreadRng is a CSPRNG seeded from OS entropy (CryptoRng); rand 0.9's
            // idiomatic default, and it avoids the rand_core version skew that the
            // libp2p dependency tree introduces around `OsRng`.
            let manifest = validator_keygen::generate_validator_keys(&params, &mut rand::rng())
                .context("generate validator keys")?;
            info!(
                count = manifest.pubkeys.len(),
                out_dir = %out_dir.display(),
                manifest = %manifest_path.display(),
                "generated validator keys",
            );
        }
    }

    Ok(Dispatch::Handled)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn dispatch_without_a_subcommand_requests_node_start() {
        let cli = Cli::try_parse_from(["lean-rust"]).expect("parse defaults");

        assert_eq!(dispatch(&cli).expect("dispatch"), Dispatch::RunNode);
    }

    #[test]
    fn dispatch_runs_devnet_config_to_completion() {
        let cli = Cli::try_parse_from(["lean-rust", "devnet-config"]).expect("parse devnet-config");

        assert_eq!(dispatch(&cli).expect("dispatch"), Dispatch::Handled);
    }

    #[test]
    fn dispatch_writes_the_generated_private_key() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let output_path = dir.path().join("node.key");
        let cli = Cli::try_parse_from([
            "lean-rust",
            "generate-private-key",
            "--output-path",
            output_path.to_str().expect("test path must be utf-8"),
        ])
        .expect("parse generate-private-key");

        assert_eq!(dispatch(&cli).expect("dispatch"), Dispatch::Handled);
        assert!(output_path.exists(), "key file must be written");
    }
}
