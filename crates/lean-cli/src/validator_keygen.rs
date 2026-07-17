//! Offline genesis keygen: per-validator XMSS attestation keys plus the
//! coordinator-canonical `genesis_validators` pubkey manifest.
//!
//! Distinct from [`keygen`](crate::keygen), which generates libp2p Ed25519 peer
//! identities. This module owns only genesis key-activation policy and manifest
//! emission; the crypto primitives come from the `crypto` port (key generation,
//! the crypto-free [`OtsKeyState`](crypto::OtsKeyState) record), never leanSig
//! internals.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crypto::{generate, ProdScheme, PublicKey};

use crate::keygen::{write_new_file, write_secret_bytes};

/// Scheme lifetime (`2^32`), sourced crypto-free from the port.
const LIFETIME: u64 = crypto::PROD_LIFETIME;

/// `sqrt(LIFETIME) = 2^16`. Activation epochs align to this boundary
/// (the XMSS spec aligns activation to `sqrt(LIFETIME)`). `isqrt` is a const fn
/// on the MSRV.
const SQRT_LIFETIME: u64 = LIFETIME.isqrt();

/// Activation-epoch duration for the shared devnet-1 keyset: `2^18 = 262144`.
///
/// This is an interop pin, expressed relative to `SQRT_LIFETIME` so no bare
/// literal appears: `2^16 << 2 = 2^18`. It is the value the shared
/// `hash-sig-cli:devnet1` keyset is generated at network-wide; the spec's
/// sqrt-min `2 * sqrt(LIFETIME) = 2^17` is the standalone floor only — keying at
/// `2^17` against the `2^18` shared keyset is a cross-client break.
const SHARED_KEYSET_ACTIVE_EPOCHS: u64 = SQRT_LIFETIME << 2;

/// The ordered genesis pubkey manifest. Position is the validator index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisKeyManifest {
    /// Public keys in ascending validator-index order.
    pub pubkeys: Vec<PublicKey>,
}

/// Parameters for one offline keygen run.
#[derive(Debug, Clone)]
pub struct KeygenParams {
    /// Number of validator keys to generate (indices `0..count`).
    pub count: u64,
    /// Directory for the per-validator `validator_<i>.ssz` secret files.
    pub out_dir: PathBuf,
    /// Output path for the `genesis_validators` manifest.
    pub manifest_path: PathBuf,
    /// Activation epoch. Must be a multiple of the sqrt-lifetime boundary
    /// (`SQRT_LIFETIME` = 2^16 = 65536) or [`generate_validator_keys`] rejects it
    /// — a misaligned epoch is refused, never silently rounded. Default 0.
    pub activation_epoch: u64,
}

impl KeygenParams {
    /// Highest `count` accepted — the on-chain validator registry cannot hold more.
    const MAX_COUNT: u64 = config::VALIDATOR_REGISTRY_LIMIT as u64;

    /// Rejects out-of-range counts and misaligned activation epochs BEFORE any
    /// (expensive) key generation.
    ///
    /// # Errors
    ///
    /// - `count` outside `1..=MAX_COUNT` (0 would emit an empty manifest; a huge
    ///   value would try to allocate before failing).
    /// - `activation_epoch` not a multiple of `SQRT_LIFETIME` — rounding it down
    ///   silently would key a different window than the operator asked for, a
    ///   quiet cross-client interop break.
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (1..=Self::MAX_COUNT).contains(&self.count),
            "count must be between 1 and {} (the validator-registry limit), got {}",
            Self::MAX_COUNT,
            self.count,
        );
        anyhow::ensure!(
            self.activation_epoch % SQRT_LIFETIME == 0,
            "activation-epoch {} must be a multiple of the sqrt-lifetime boundary {} \
             (2^16); pick an aligned epoch",
            self.activation_epoch,
            SQRT_LIFETIME,
        );
        Ok(())
    }
}

/// Generates `count` validator keys (ascending index is the canonical order),
/// writes each secret as `<out_dir>/validator_<i>.ssz`, writes the canonical
/// manifest to `manifest_path`, and returns the in-memory manifest.
///
/// The RNG is injected so tests are deterministic with a seeded RNG while
/// production passes a `ThreadRng` (`rand::rng()`, a `CryptoRng`; DIP).
/// `crypto::generate` samples a per-key seed from it and retains that seed in the
/// persisted record.
///
/// # Errors
///
/// Returns an error if `params` fail [`KeygenParams::validate`], a key cannot be
/// generated, a secret or manifest file already exists (`create_new` refuses to
/// clobber), or a write fails.
pub fn generate_validator_keys<R: rand::RngCore + rand::CryptoRng>(
    params: &KeygenParams,
    rng: &mut R,
) -> Result<GenesisKeyManifest> {
    params.validate()?;

    // Fail fast on ANY pre-existing output BEFORE the expensive keygen loop — a
    // mistargeted path or a leftover from an aborted run should not burn keygen
    // time or leave a partial result. `create_new` remains the authoritative
    // no-clobber guard for the writes themselves. `count` is bounded (<= registry
    // limit) by validate(), so enumerating the paths is cheap.
    anyhow::ensure!(
        !params.manifest_path.exists(),
        "output {} already exists; remove it (or the out-dir) to regenerate",
        params.manifest_path.display(),
    );
    for index in 0..params.count {
        let path = secret_path(&params.out_dir, index);
        anyhow::ensure!(
            !path.exists(),
            "output {} already exists; remove it (or the out-dir) to regenerate",
            path.display(),
        );
    }

    let activation =
        usize::try_from(params.activation_epoch).context("activation epoch exceeds usize")?;
    let active_epochs =
        usize::try_from(SHARED_KEYSET_ACTIVE_EPOCHS).context("active epochs exceed usize")?;

    // `count` is validated to `<= MAX_COUNT`, so this allocation is bounded.
    let mut pubkeys = Vec::with_capacity(usize::try_from(params.count).unwrap_or(0));
    for index in 0..params.count {
        let (pubkey, signing_key) = generate::<ProdScheme, _>(rng, activation, active_epochs)
            .with_context(|| format!("generate validator key {index}"))?;

        // SA5: crypto-free record (seed, activation window, next_index=0 at
        // genesis). Fixed 56-byte inherent layout; own-and-move, no secret clone.
        let record = signing_key.to_record();
        let path = secret_path(&params.out_dir, index);
        write_secret_bytes(&path, &record.to_ssz_bytes())
            .with_context(|| format!("write validator secret {}", path.display()))?;

        pubkeys.push(pubkey);
    }

    // The manifest carries only PUBLIC pubkeys and is a shared interop artifact —
    // write it world-readable (default perms), NOT owner-only like the secrets.
    let manifest = GenesisKeyManifest { pubkeys };
    write_new_file(
        &params.manifest_path,
        serialize_manifest(&manifest).as_bytes(),
    )
    .with_context(|| format!("write manifest {}", params.manifest_path.display()))?;

    Ok(manifest)
}

/// `<out_dir>/validator_<index>.ssz`.
fn secret_path(out_dir: &Path, index: u64) -> PathBuf {
    out_dir.join(format!("validator_{index}.ssz"))
}

/// Canonical manifest text: a `genesis_validators:` YAML sequence of lower-case
/// hex pubkeys, index-ascending, one per line, no trailing whitespace.
///
/// A pure function of the ordered pubkeys — the interop byte-identity guarantee.
/// No map iteration on this path.
#[must_use]
fn serialize_manifest(manifest: &GenesisKeyManifest) -> String {
    // An empty sequence must serialize as `[]`, not a bare key (which YAML reads
    // as null, breaking a `Vec` deserializer). generate_validator_keys rejects
    // count 0, so this is defensive.
    if manifest.pubkeys.is_empty() {
        return String::from("genesis_validators: []\n");
    }
    let mut out = String::from("genesis_validators:\n");
    for pubkey in &manifest.pubkeys {
        out.push_str("  - ");
        out.push_str(&hex::encode(pubkey.as_slice())); // hex::encode is lower-case
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn seeded() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    fn params(dir: &Path, count: u64) -> KeygenParams {
        KeygenParams {
            count,
            out_dir: dir.join("secrets"),
            manifest_path: dir.join("genesis_validators.yaml"),
            activation_epoch: 0,
        }
    }

    #[test]
    fn activation_constants_and_param_validation() {
        assert_eq!(SQRT_LIFETIME, 1 << 16);
        assert_eq!(SHARED_KEYSET_ACTIVE_EPOCHS, 1 << 18); // 2^18, NOT 2^17

        // Aligned activation validates; a misaligned one is REJECTED (no silent
        // rounding), and count must be in 1..=MAX_COUNT.
        let base = params(Path::new("/tmp/x"), 1);
        assert!(KeygenParams {
            activation_epoch: 1 << 16,
            ..base.clone()
        }
        .validate()
        .is_ok());
        assert!(KeygenParams {
            activation_epoch: (1 << 16) + 5,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(KeygenParams {
            count: 0,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(KeygenParams {
            count: KeygenParams::MAX_COUNT + 1,
            ..base
        }
        .validate()
        .is_err());
    }

    #[test]
    fn empty_manifest_serializes_as_yaml_sequence() {
        // Defensive: an empty list must be `[]` (a sequence), not a bare key that
        // YAML reads as null. Directly unit-tested since count 0 is rejected upstream.
        let empty = GenesisKeyManifest { pubkeys: vec![] };
        assert_eq!(serialize_manifest(&empty), "genesis_validators: []\n");
    }

    // One count=2 keygen pair covers len==count, 52-byte elements, index-ascending
    // order, canonical format, AND byte-identity for a fixed seed — folded together
    // because each ProdScheme keygen is expensive.
    #[test]
    fn manifest_shape_ordering_and_byte_identity() {
        let dir_a = tempfile::tempdir().expect("tempdir");
        let dir_b = tempfile::tempdir().expect("tempdir");
        let pa = params(dir_a.path(), 2);
        let a = generate_validator_keys(&pa, &mut seeded()).expect("keygen a");
        let b = generate_validator_keys(&params(dir_b.path(), 2), &mut seeded()).expect("keygen b");

        // len == count; every element is a 52-byte PublicKey.
        assert_eq!(a.pubkeys.len(), 2);
        assert_eq!(PublicKey::LEN, 52);
        for pk in &a.pubkeys {
            assert_eq!(pk.as_slice().len(), PublicKey::LEN);
        }

        // Each index gets its own secret file: validator_0.ssz, validator_1.ssz.
        assert!(secret_path(&pa.out_dir, 0).is_file());
        assert!(secret_path(&pa.out_dir, 1).is_file());

        // Same seed -> byte-identical manifest. NOTE: production uses ThreadRng, so
        // seed-determinism is a TEST property; the network interop invariants are
        // the manifest FORMAT + index ordering + the 2^18 duration.
        let text = serialize_manifest(&a);
        assert_eq!(text, serialize_manifest(&b));

        // Canonical format: header, then row i == validator index i's lower-case
        // hex pubkey, no trailing whitespace, no tabs.
        assert!(text.starts_with("genesis_validators:\n"));
        assert!(!text.contains('\t'));
        let rows: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(rows.len(), a.pubkeys.len());
        for (i, line) in rows.iter().enumerate() {
            assert_eq!(*line, line.trim_end(), "no trailing whitespace");
            let hexpart = line.trim_start_matches("  - ");
            assert_eq!(hexpart, hexpart.to_lowercase(), "lower-case hex");
            assert_eq!(
                hexpart,
                hex::encode(a.pubkeys[i].as_slice()),
                "row {i} is index {i}"
            );
        }
    }

    #[test]
    fn secret_material_written_as_otskeystate_ssz_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = params(dir.path(), 1);
        generate_validator_keys(&p, &mut seeded()).expect("keygen");
        let path = secret_path(&p.out_dir, 0);
        let bytes = std::fs::read(&path).expect("read secret");
        // SA5 round-trip: decode == the persisted bytes (fixed 56-byte record).
        let record = crypto::OtsKeyState::from_ssz_bytes(&bytes).expect("decode");
        assert_eq!(record.to_ssz_bytes().as_slice(), bytes.as_slice());
        assert_eq!(record.next_index, 0); // genesis: nothing signed yet
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn keygen_refuses_to_overwrite_existing_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = params(dir.path(), 1);
        // Pre-seed validator_0's secret so the run refuses to clobber it. The
        // fail-fast pre-check catches the existing file before any keygen runs;
        // create_new remains the authoritative guard for the writes themselves.
        std::fs::create_dir_all(&p.out_dir).expect("mkdir");
        std::fs::write(secret_path(&p.out_dir, 0), b"existing").expect("seed file");
        let err = generate_validator_keys(&p, &mut seeded()).unwrap_err();
        // Assert specifically on the clobber signal — the outer "validator secret"
        // context is always present, so it cannot distinguish a no-clobber refusal
        // from any other write failure.
        assert!(
            err.chain()
                .any(|c| c.to_string().contains("already exists")),
            "run must refuse to clobber an existing secret (create_new): {err:?}"
        );
    }

    #[test]
    fn generated_key_signs_and_verifies_sample_htr() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = params(dir.path(), 1);
        generate_validator_keys(&p, &mut seeded()).expect("keygen");

        // Reload the secret through the SA5 record and sign a sample HTR; the
        // manifest pubkey must verify it (matched, usable pair). Genesis activation
        // includes epoch 0, which is in the reloaded key's initial window, so
        // from_record + sign(0) works without an explicit prepare.
        let bytes = std::fs::read(secret_path(&p.out_dir, 0)).expect("read");
        let record = crypto::OtsKeyState::from_ssz_bytes(&bytes).expect("decode");
        let mut signing_key =
            crypto::SigningKey::<ProdScheme>::from_record(&record).expect("from_record");

        let manifest_text = std::fs::read_to_string(&p.manifest_path).expect("read manifest");
        let hexpk = manifest_text
            .lines()
            .nth(1)
            .expect("row")
            .trim_start_matches("  - ");
        let raw: [u8; PublicKey::LEN] = hex::decode(hexpk)
            .expect("hex")
            .try_into()
            .expect("52 bytes");
        let pk = PublicKey::new(raw);

        let htr = [0xa5u8; 32]; // sample hash_tree_root
        let epoch = 0u32;
        let sig = signing_key.sign(epoch, &htr).expect("sign");
        assert!(crypto::verify::<ProdScheme>(&pk, epoch, &htr, &sig).is_ok());

        // Bound to the exact message: a different HTR does not verify.
        let mut other = htr;
        other[0] ^= 0x01;
        assert!(crypto::verify::<ProdScheme>(&pk, epoch, &other, &sig).is_err());
    }
}
