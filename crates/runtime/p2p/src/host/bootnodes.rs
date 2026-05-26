//! Bootnodes YAML loader.
//!
//! Wire format: flat YAML list of multiaddr strings whose terminal
//! component is `/p2p/<peer-id>`.
//!
//! ```yaml
//! - /ip4/192.0.2.10/udp/9000/quic-v1/p2p/12D3KooW...
//! - /ip4/192.0.2.11/udp/9000/quic-v1/p2p/12D3KooW...
//! ```
//!
//! The local-pq devnet currently generates ENR `nodes.yaml` for ream
//! consumers and a temporary `bootnodes.rust.yaml` adapter in this same
//! flat multiaddr shape for Rust. Native ENR parsing can replace that
//! adapter later without broadening this loader's wire contract.
//!
//! Each entry parses into a `(Multiaddr, PeerId)` pair: the swarm dials
//! the multiaddr; the peer id is required for outbound identification
//! before the libp2p handshake completes.

use std::fs;

use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};
use tracing::debug;

use crate::error::{HostError, HostResult};
use crate::options::BootnodesPath;

/// A parsed bootnode entry: dialable multiaddr + peer id pulled off the
/// terminal `/p2p/<peer-id>` component.
#[derive(Debug)]
pub struct Bootnode {
    /// Dialable multiaddr stripped of the trailing `/p2p/<peer-id>`
    /// component.
    pub addr: Multiaddr,
    /// Peer id extracted from the trailing `/p2p/<peer-id>` component.
    pub peer_id: PeerId,
}

/// Loads the YAML file at `path` and returns parsed bootnodes.
///
/// An empty file or empty YAML sequence resolves to an empty `Vec`.
///
/// # Errors
/// - [`HostError::BootnodesRead`] when the file cannot be read.
/// - [`HostError::BootnodesParse`] when the YAML shape is wrong.
/// - [`HostError::InvalidBootnode`] when a single entry fails the
///   multiaddr + `/p2p/<peer-id>` check.
pub fn load(path: &BootnodesPath) -> HostResult<Vec<Bootnode>> {
    let p = path.as_path();
    let bytes = fs::read(p).map_err(|source| HostError::BootnodesRead {
        path: p.to_path_buf(),
        source,
    })?;
    let raw: Vec<String> = parse_yaml(&bytes).map_err(|source| HostError::BootnodesParse {
        path: p.to_path_buf(),
        source,
    })?;
    debug!(
        path = %p.display(),
        bytes = bytes.len(),
        entries = raw.len(),
        "read bootnodes YAML",
    );
    raw.into_iter().map(parse_entry).collect()
}

fn parse_yaml(bytes: &[u8]) -> Result<Vec<String>, serde_yaml::Error> {
    // Empty file → YAML `null` → treat as empty list.
    let parsed: Option<Vec<String>> = serde_yaml::from_slice(bytes)?;
    Ok(parsed.unwrap_or_default())
}

fn parse_entry(entry: String) -> HostResult<Bootnode> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return Err(invalid_entry(entry, "entry is empty or whitespace-only"));
    }
    let mut addr = match trimmed.parse::<Multiaddr>() {
        Ok(addr) => addr,
        Err(source) => {
            return Err(invalid_entry(
                entry,
                format!("not a valid multiaddr: {source}"),
            ));
        }
    };
    let peer_id = match addr.pop() {
        Some(Protocol::P2p(peer_id)) => peer_id,
        Some(other) => {
            return Err(invalid_entry(
                entry,
                format!("terminal component must be /p2p/<peer-id>, got {other}"),
            ));
        }
        None => {
            return Err(invalid_entry(entry, "multiaddr is empty after parse"));
        }
    };
    Ok(Bootnode { addr, peer_id })
}

fn invalid_entry(entry: String, reason: impl Into<String>) -> HostError {
    HostError::InvalidBootnode {
        entry,
        reason: reason.into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{DevnetHost, HostOptions};
    use fixtures::{rust_bootnodes_2node_path, REAM_0_BOOTNODE_ADDR, REAM_0_PEER_ID};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn fixture(contents: &str) -> (NamedTempFile, BootnodesPath) {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        let path = BootnodesPath::new(file.path()).unwrap();
        (file, path)
    }

    fn known_good_entry() -> String {
        // Construct from a freshly generated keypair so the peer-id
        // component is guaranteed valid across libp2p versions without
        // hardcoding a literal that future libp2p versions might reject.
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer = keypair.public().to_peer_id();
        format!("/ip4/127.0.0.1/udp/9000/quic-v1/p2p/{peer}")
    }

    fn assert_invalid(err: &HostError) {
        assert!(
            matches!(err, HostError::InvalidBootnode { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn loads_well_formed_entries() {
        let entry = known_good_entry();
        let (_file, path) = fixture(&format!("- {entry}\n"));

        let nodes = load(&path).unwrap();
        assert_eq!(nodes.len(), 1);
        // The pop'd peer id round-trips through the formatter.
        let recomposed = format!("{}/p2p/{}", nodes[0].addr, nodes[0].peer_id);
        assert_eq!(recomposed, entry);
    }

    #[test]
    fn loads_local_pq_rust_bootnodes_adapter_fixture() {
        let path = BootnodesPath::new(rust_bootnodes_2node_path()).unwrap();

        let nodes = load(&path).unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].addr.to_string(), REAM_0_BOOTNODE_ADDR);
        assert_eq!(nodes[0].peer_id.to_string(), REAM_0_PEER_ID);
    }

    #[test]
    fn host_build_accepts_local_pq_rust_bootnodes_adapter_path() {
        let identity_dir = tempfile::tempdir().unwrap();
        let adapter_path = rust_bootnodes_2node_path();
        let options = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &identity_dir.path().join("identity.pb"),
            Some(&adapter_path),
        )
        .unwrap();

        let host = DevnetHost::build(options).unwrap();

        assert_ne!(host.peer_id().to_string(), REAM_0_PEER_ID);
    }

    #[test]
    fn empty_file_resolves_to_empty_list() {
        let (_file, path) = fixture("");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn empty_sequence_resolves_to_empty_list() {
        let (_file, path) = fixture("[]\n");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn malformed_yaml_surfaces_parse_error() {
        let (_file, path) = fixture("not: [a, valid, list\n");
        let err = load(&path).unwrap_err();
        assert!(
            matches!(err, HostError::BootnodesParse { .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn missing_file_surfaces_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = BootnodesPath::new(dir.path().join("nonexistent.yaml")).unwrap();
        let err = load(&path).unwrap_err();
        assert!(
            matches!(err, HostError::BootnodesRead { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_entry_without_peer_id_suffix() {
        // No /p2p/<peer-id> component at the end.
        let (_file, path) = fixture("- /ip4/127.0.0.1/udp/9000/quic-v1\n");
        assert_invalid(&load(&path).unwrap_err());
    }

    #[test]
    fn rejects_unparseable_entry() {
        let (_file, path) = fixture("- not-a-multiaddr\n");
        let err = load(&path).unwrap_err();
        let HostError::InvalidBootnode { entry, .. } = err else {
            panic!("expected InvalidBootnode");
        };
        assert_eq!(entry, "not-a-multiaddr");
    }

    #[test]
    fn rejects_empty_string_entry() {
        let (_file, path) = fixture("- \"\"\n");
        assert_invalid(&load(&path).unwrap_err());
    }
}
