//! Persistent libp2p identity (protobuf-encoded Ed25519 keypair).
//!
//! Missing file → generate, persist (mode `0600` on POSIX), return.
//! Present file → decode, return. Corrupt bytes are never silently
//! overwritten — surfaced as [`HostError::InvalidIdentity`] so the
//! caller (operator) decides whether to remove and regenerate.

use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use libp2p::identity::Keypair;
use tracing::{debug, info};

use crate::error::{HostError, HostResult};
use crate::options::IdentityPath;

/// Loads the keypair from disk, generating + persisting a fresh
/// Ed25519 keypair if the file does not exist.
///
/// # Errors
/// - [`HostError::IdentityIo`] on file read / write / chmod failure.
/// - [`HostError::InvalidIdentity`] when an existing file is present
///   but its bytes do not decode as a libp2p protobuf-encoded keypair.
pub fn load_or_generate(path: &IdentityPath) -> HostResult<Keypair> {
    let p = path.as_path();
    match fs::read(p) {
        Ok(bytes) => {
            let keypair = Keypair::from_protobuf_encoding(&bytes).map_err(|source| {
                HostError::InvalidIdentity {
                    path: p.to_path_buf(),
                    source,
                }
            })?;
            debug!(
                path = %p.display(),
                peer_id = %keypair.public().to_peer_id(),
                "loaded existing host identity",
            );
            Ok(keypair)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => generate_and_persist(path),
        Err(source) => Err(io_err(p, source)),
    }
}

fn generate_and_persist(path: &IdentityPath) -> HostResult<Keypair> {
    let p = path.as_path();
    let keypair = Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        // `to_protobuf_encoding` only errors when the keypair carries a
        // variant lacking a protobuf representation. `generate_ed25519`
        // always produces an Ed25519 keypair, which has one — so this
        // arm is unreachable in practice. Surfaced as `IdentityIo` for
        // safety rather than panicking.
        .map_err(|source| io_err(p, std::io::Error::other(source.to_string())))?;

    if let Some(parent) = p.parent().filter(|q| !q.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|source| io_err(parent, source))?;
    }

    write_identity_bytes(p, &bytes)?;

    info!(
        path = %p.display(),
        peer_id = %keypair.public().to_peer_id(),
        "generated new host identity",
    );
    Ok(keypair)
}

/// On Unix, atomically creates the identity file with mode `0o600` so
/// the keypair is never world-readable in the window between creation
/// and a follow-up `chmod`. On non-Unix targets, falls back to
/// [`fs::write`] (the platform's permission model differs and is not
/// part of this crate's threat model).
#[cfg(unix)]
fn write_identity_bytes(path: &Path, bytes: &[u8]) -> HostResult<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| io_err(path, source))?;
    file.write_all(bytes).map_err(|source| io_err(path, source))
}

#[cfg(not(unix))]
fn write_identity_bytes(path: &Path, bytes: &[u8]) -> HostResult<()> {
    fs::write(path, bytes).map_err(|source| io_err(path, source))
}

fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> HostError {
    HostError::IdentityIo {
        path: path.into(),
        source,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::{tempdir, TempDir};

    fn temp_identity_at(rel: impl AsRef<Path>) -> (TempDir, IdentityPath) {
        let dir = tempdir().unwrap();
        let path = IdentityPath::new(dir.path().join(rel)).unwrap();
        (dir, path)
    }

    fn temp_identity_path() -> (TempDir, IdentityPath) {
        temp_identity_at("p2p_priv_key")
    }

    #[test]
    fn missing_file_generates_and_persists() {
        let (_dir, path) = temp_identity_path();

        let first = load_or_generate(&path).unwrap();
        assert!(path.as_path().exists());

        let reloaded = load_or_generate(&path).unwrap();
        assert_eq!(
            first.public().to_peer_id(),
            reloaded.public().to_peer_id(),
            "second load must return the persisted identity, not regenerate",
        );
    }

    #[test]
    fn corrupt_bytes_surface_invalid_identity_without_overwrite() {
        let (_dir, path) = temp_identity_path();
        let corrupt = b"not-a-protobuf-keypair";
        fs::write(path.as_path(), corrupt).unwrap();

        let err = load_or_generate(&path).unwrap_err();
        assert!(
            matches!(err, HostError::InvalidIdentity { .. }),
            "got {err:?}",
        );

        let on_disk = fs::read(path.as_path()).unwrap();
        assert_eq!(
            on_disk, corrupt,
            "corrupt identity file must never be silently overwritten",
        );
    }

    #[cfg(unix)]
    #[test]
    fn generated_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, path) = temp_identity_path();
        load_or_generate(&path).unwrap();

        let mode = fs::metadata(path.as_path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600, got {mode:o}");
    }

    #[test]
    fn creates_parent_directory_when_missing() {
        let (_dir, path) = temp_identity_at("deeply/nested/id");
        load_or_generate(&path).unwrap();
        assert!(path.as_path().exists());
    }
}
