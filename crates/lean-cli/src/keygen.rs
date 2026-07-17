//! libp2p private-key generation for `lean-rust`.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use libp2p::{identity::Keypair, PeerId};
use runtime::p2p::{load_existing_peer_id, IdentityPath};

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

    write_secret_bytes(output_path, &bytes)?;
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

/// Writes `bytes` to `path` as an owner-only (`0o600`), `create_new` secret file,
/// creating parent directories as needed.
///
/// `create_new` refuses to overwrite an existing file, so a re-run cannot silently
/// destroy key material. Shared by libp2p identity keygen and XMSS validator keygen.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created, the file already
/// exists, or the write / permission-adjust fails.
pub(crate) fn write_secret_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent_dir(path)?;
    // `owner_only` sets the mode atomically at creation (unix); the explicit
    // set_owner_only_permissions below is belt-and-suspenders against umask.
    let file = open_create_new(path, /* owner_only */ true)?;
    write_all_flush(file, path, bytes)?;
    set_owner_only_permissions(path)?;
    Ok(())
}

/// Writes `bytes` to a NEW file at `path` (`create_new`, no-clobber) with default
/// permissions, creating parent directories as needed.
///
/// For PUBLIC artifacts that another user or a different uid must read — e.g. the
/// `genesis_validators` pubkey manifest, which is a shared interop artifact. Unlike
/// [`write_secret_bytes`], it does NOT restrict the file to the owner.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created, the file already
/// exists, or the write fails.
pub(crate) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent_dir(path)?;
    let file = open_create_new(path, /* owner_only */ false)?;
    write_all_flush(file, path, bytes)
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create key output directory {}", parent.display()))?;
    }
    Ok(())
}

/// Context message for a failed `create_new` open — names the path and hints that
/// an already-existing file must be removed. The "already exists" phrasing is
/// load-bearing: the no-clobber test asserts on it.
fn open_context(path: &Path) -> String {
    format!(
        "open output file {} (already exists? delete it first to regenerate)",
        path.display()
    )
}

/// Writes then flushes `bytes`, attaching path context to each step.
fn write_all_flush(mut file: fs::File, path: &Path, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .with_context(|| format!("write output file {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush output file {}", path.display()))
}

/// Opens `path` with `create_new` (`O_CREAT | O_EXCL`) so an existing file is never
/// clobbered. When `owner_only`, the file is created `0o600` atomically (unix).
#[cfg(unix)]
fn open_create_new(path: &Path, owner_only: bool) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut opts = fs::OpenOptions::new();
    opts.create_new(true).write(true);
    if owner_only {
        opts.mode(0o600);
    }
    opts.open(path).with_context(|| open_context(path))
}

#[cfg(not(unix))]
fn open_create_new(path: &Path, _owner_only: bool) -> Result<fs::File> {
    // No unix mode bits here; set_owner_only_permissions is also a no-op on
    // non-unix, so secret files are NOT owner-restricted on these targets.
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| open_context(path))
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
