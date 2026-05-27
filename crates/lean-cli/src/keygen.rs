//! libp2p private-key generation for `lean-rust`.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use lean_p2p_host::{load_existing_peer_id, IdentityPath};
use libp2p::{identity::Keypair, PeerId};

/// Generates a new Ed25519 libp2p keypair and writes it to `output_path`.
///
/// The file contains libp2p's protobuf-encoded private-key bytes. Parent
/// directories are created when needed. On Unix, the resulting file mode is
/// forced to `0o600`.
///
/// # Errors
///
/// Returns an error if key encoding fails or the output path cannot be
/// created, written, flushed, or permission-adjusted.
pub fn generate_and_write(output_path: &Path) -> Result<PeerId> {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();
    let bytes = keypair
        .to_protobuf_encoding()
        .context("encode libp2p keypair as protobuf")?;

    write_key_bytes(output_path, &bytes)?;
    Ok(peer_id)
}

/// Loads an existing libp2p identity file and returns its peer ID.
///
/// The file may contain either libp2p protobuf-encoded key material or
/// local-pq raw hex secp256k1 key material.
///
/// # Errors
///
/// Returns an error if the path is invalid, the file cannot be read, or
/// the key material cannot be decoded.
pub fn peer_id_from_file(path: &Path) -> Result<PeerId> {
    let identity_path = IdentityPath::new(path).context("validate identity path")?;
    load_existing_peer_id(&identity_path)
        .with_context(|| format!("load identity file {}", path.display()))
}

fn write_key_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create key output directory {}", parent.display()))?;
    }

    write_file(path, bytes)?;
    set_owner_only_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    // create_new (O_CREAT | O_EXCL) refuses to overwrite an existing key so a
    // re-run of `generate-private-key` cannot silently destroy a validator
    // identity. Mirrors host::keypair's own write path.
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| {
            format!(
                "open key output file {} (file already exists? delete it first if you really mean to replace the validator identity)",
                path.display()
            )
        })?;
    file.write_all(bytes)
        .with_context(|| format!("write key output file {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush key output file {}", path.display()))
}

#[cfg(not(unix))]
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::ErrorKind;

    // create_new refuses to overwrite an existing key. fs::write would
    // silently truncate.
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => anyhow::anyhow!(
                "key output file {} already exists (delete it first if you really mean to replace the validator identity)",
                path.display()
            ),
            _ => anyhow::Error::new(e).context(format!("open key output file {}", path.display())),
        })?;
    file.write_all(bytes)
        .with_context(|| format!("write key output file {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush key output file {}", path.display()))
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set key output permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn read_key(path: &Path) -> Keypair {
        let bytes = fs::read(path).expect("read generated key");
        Keypair::from_protobuf_encoding(&bytes).expect("decode generated key")
    }

    #[test]
    fn generate_and_write_persists_reloadable_key() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("keys/node.pb");

        let peer_id = generate_and_write(&path).expect("generate key");
        let reloaded = read_key(&path);

        assert_eq!(peer_id, reloaded.public().to_peer_id());
        assert_ne!(fs::metadata(path).expect("read key metadata").len(), 0);
    }

    #[test]
    fn peer_id_from_file_loads_generated_key() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("node.pb");

        let generated = generate_and_write(&path).expect("generate key");
        let loaded = peer_id_from_file(&path).expect("load peer id");

        assert_eq!(loaded, generated);
    }

    #[test]
    fn peer_id_from_file_loads_local_pq_raw_secp256k1_key() {
        let peer_id =
            peer_id_from_file(&fixtures::ream_0_raw_secp256k1_key_path()).expect("load peer id");

        assert_eq!(peer_id.to_string(), fixtures::REAM_0_PEER_ID);
    }

    #[cfg(unix)]
    #[test]
    fn generated_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("node.pb");

        generate_and_write(&path).expect("generate key");

        let mode = fs::metadata(path)
            .expect("read key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
