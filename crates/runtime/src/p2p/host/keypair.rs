//! Persistent libp2p keypairs.
//!
//! Missing file → generate, persist (mode `0600` on POSIX), return.
//! Present file → decode as protobuf or local-pq raw secp256k1 hex,
//! return. Corrupt bytes are never silently overwritten so the caller
//! decides whether to remove and regenerate.

use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use libp2p::{
    identity::{secp256k1, Keypair},
    PeerId,
};
use tracing::{debug, info};

use crate::p2p::error::{HostError, HostResult};
use crate::p2p::options::IdentityPath;

const RAW_SECP256K1_PRIVATE_KEY_LEN: usize = 32;

/// Loads the keypair from disk, generating + persisting a fresh
/// Ed25519 keypair if the file does not exist.
///
/// # Errors
/// - [`HostError::IdentityIo`] on file read / write / chmod failure.
/// - [`HostError::InvalidRawIdentityHex`] when an existing file is not
///   valid raw secp256k1 hex.
/// - [`HostError::InvalidRawIdentityLength`] when raw hex does not decode
///   to a 32-byte secp256k1 private key.
/// - [`HostError::InvalidRawIdentityKeyMaterial`] when raw bytes are not
///   valid secp256k1 secret-key material.
pub fn load_or_generate(path: &IdentityPath) -> HostResult<Keypair> {
    let p = path.as_path();
    match fs::read(p) {
        Ok(bytes) => decode_existing(p, &bytes),
        Err(err) if err.kind() == ErrorKind::NotFound => generate_and_persist(path),
        Err(source) => Err(io_err(p, source)),
    }
}

/// Loads the peer ID from an existing identity file without generating a
/// replacement.
///
/// # Errors
/// - [`HostError::IdentityIo`] when the file cannot be read.
/// - [`HostError::InvalidRawIdentityHex`] when the file is neither
///   protobuf nor valid raw secp256k1 hex.
/// - [`HostError::InvalidRawIdentityLength`] when raw hex does not decode
///   to a 32-byte secp256k1 private key.
/// - [`HostError::InvalidRawIdentityKeyMaterial`] when raw secp256k1 bytes
///   are not valid secret-key material.
pub fn load_existing_peer_id(path: &IdentityPath) -> HostResult<PeerId> {
    load_existing(path).map(|keypair| keypair.public().to_peer_id())
}

fn load_existing(path: &IdentityPath) -> HostResult<Keypair> {
    let p = path.as_path();
    let bytes = fs::read(p).map_err(|source| io_err(p, source))?;
    decode_existing(p, &bytes)
}

fn decode_existing(path: &Path, bytes: &[u8]) -> HostResult<Keypair> {
    let keypair = if let Ok(keypair) = Keypair::from_protobuf_encoding(bytes) {
        debug!(
            path = %path.display(),
            peer_id = %keypair.public().to_peer_id(),
            "loaded existing protobuf host identity",
        );
        keypair
    } else {
        let keypair = decode_raw_secp256k1_hex(path, bytes)?;
        debug!(
            path = %path.display(),
            peer_id = %keypair.public().to_peer_id(),
            "loaded existing raw secp256k1 host identity",
        );
        keypair
    };
    Ok(keypair)
}

fn decode_raw_secp256k1_hex(path: &Path, bytes: &[u8]) -> HostResult<Keypair> {
    let raw = std::str::from_utf8(bytes).map_err(|source| HostError::InvalidRawIdentityHex {
        path: path.to_path_buf(),
        reason: source.to_string(),
    })?;
    let mut secret_bytes =
        hex::decode(raw.trim()).map_err(|source| HostError::InvalidRawIdentityHex {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
    if secret_bytes.len() != RAW_SECP256K1_PRIVATE_KEY_LEN {
        return Err(HostError::InvalidRawIdentityLength {
            path: path.to_path_buf(),
            actual: secret_bytes.len(),
            expected: RAW_SECP256K1_PRIVATE_KEY_LEN,
        });
    }

    let secret = secp256k1::SecretKey::try_from_bytes(&mut secret_bytes).map_err(|source| {
        HostError::InvalidRawIdentityKeyMaterial {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(Keypair::from(secp256k1::Keypair::from(secret)))
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
    use crate::p2p::{DevnetHost, HostOptions};
    use fixtures::{leanrust_1_raw_secp256k1_key_path, LEANRUST_1_PEER_ID};
    use tempfile::{tempdir, TempDir};

    fn temp_identity_at(rel: impl AsRef<Path>) -> (TempDir, IdentityPath) {
        let dir = tempdir().unwrap();
        let path = IdentityPath::new(dir.path().join(rel)).unwrap();
        (dir, path)
    }

    fn temp_identity_path() -> (TempDir, IdentityPath) {
        temp_identity_at("p2p_priv_key")
    }

    fn raw_fixture_path() -> PathBuf {
        leanrust_1_raw_secp256k1_key_path()
    }

    fn raw_fixture_bytes() -> Vec<u8> {
        fs::read(raw_fixture_path()).expect("read raw secp256k1 fixture")
    }

    #[test]
    fn load_existing_missing_file_does_not_generate() {
        let (dir, path) = temp_identity_path();

        let err = load_existing(&path).unwrap_err();

        assert!(matches!(err, HostError::IdentityIo { .. }), "got {err:?}");
        assert!(!dir.path().join("p2p_priv_key").exists());
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
    fn invalid_hex_surfaces_explicit_error_without_overwrite() {
        let (_dir, path) = temp_identity_path();
        let corrupt = b"not-a-protobuf-keypair";
        fs::write(path.as_path(), corrupt).unwrap();

        let err = load_or_generate(&path).unwrap_err();
        match err {
            HostError::InvalidRawIdentityHex { path: err_path, .. } => {
                assert_eq!(err_path, path.as_path());
            }
            other => panic!("expected invalid raw hex, got {other:?}"),
        }

        let on_disk = fs::read(path.as_path()).unwrap();
        assert_eq!(
            on_disk, corrupt,
            "corrupt identity file must never be silently overwritten",
        );
    }

    #[test]
    fn raw_secp256k1_hex_fixture_loads_stable_peer_id() {
        let path = IdentityPath::new(raw_fixture_path()).unwrap();

        let keypair = load_or_generate(&path).unwrap();

        assert_eq!(
            keypair.public().to_peer_id().to_string(),
            LEANRUST_1_PEER_ID
        );
    }

    #[test]
    fn raw_secp256k1_hex_file_is_not_rewritten() {
        let (_dir, path) = temp_identity_at("node1.key");
        let raw = raw_fixture_bytes();
        fs::write(path.as_path(), &raw).unwrap();

        load_or_generate(&path).unwrap();

        let on_disk = fs::read(path.as_path()).unwrap();
        assert_eq!(on_disk, raw);
    }

    #[test]
    fn raw_secp256k1_hex_wrong_length_is_explicit() {
        let (_dir, path) = temp_identity_path();
        fs::write(path.as_path(), b"aa").unwrap();

        let err = load_or_generate(&path).unwrap_err();

        match err {
            HostError::InvalidRawIdentityLength {
                path: err_path,
                actual,
                expected,
            } => {
                assert_eq!(err_path, path.as_path());
                assert_eq!(actual, 1);
                assert_eq!(expected, RAW_SECP256K1_PRIVATE_KEY_LEN);
            }
            other => panic!("expected raw key length error, got {other:?}"),
        }
    }

    #[test]
    fn raw_secp256k1_invalid_key_material_is_explicit() {
        let (_dir, path) = temp_identity_path();
        fs::write(path.as_path(), "00".repeat(RAW_SECP256K1_PRIVATE_KEY_LEN)).unwrap();

        let err = load_or_generate(&path).unwrap_err();

        match err {
            HostError::InvalidRawIdentityKeyMaterial { path: err_path, .. } => {
                assert_eq!(err_path, path.as_path());
            }
            other => panic!("expected raw key material error, got {other:?}"),
        }
    }

    #[test]
    fn host_build_loads_raw_secp256k1_identity_path() {
        let (_dir, path) = temp_identity_at("node1.key");
        fs::write(path.as_path(), raw_fixture_bytes()).unwrap();
        let options = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            path.as_path(),
            None,
        )
        .unwrap();

        let host = DevnetHost::build(options).unwrap();

        assert_eq!(host.peer_id().to_string(), LEANRUST_1_PEER_ID);
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
