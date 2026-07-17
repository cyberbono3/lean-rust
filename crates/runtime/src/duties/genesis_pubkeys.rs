//! Genesis pubkey manifest → validator registry.
//!
//! Parses the coordinator-canonical `genesis_validators.yaml` manifest emitted
//! by the offline keygen tool (a `genesis_validators:` YAML sequence of
//! index-ascending, unprefixed lower-case hex pubkeys) and zips it against a
//! loaded [`ValidatorAssignments`] to build the ordered `Validator` registry
//! that genesis assembly populates into `State.validators`.
//!
//! Signature-free: pubkeys are plain 52-byte wire values (`types::PublicKey`);
//! this loader never imports the signing adapter and performs no sign/verify.

use std::io;
use std::path::Path;

use protocol::{Validator, ValidatorIndex};
use serde::Deserialize;
use types::PublicKey;

use super::error::{DutiesError, DutiesResult};
use super::validators::{read_capped, resolve_path, ValidatorAssignments};

/// Raw manifest shape — a `genesis_validators:` sequence of hex strings.
///
/// A serde DTO, not a domain entity — the [`Validator`] registry it produces
/// stays serde-free.
#[derive(Debug, Deserialize)]
struct RawManifest {
    genesis_validators: Vec<String>,
}

/// The ordered genesis validator registry: index-ascending and contiguous over
/// `0..total_validators`, so genesis `State.validators` hash-tree-root is
/// deterministic across clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisRegistry {
    validators: Vec<Validator>,
}

impl GenesisRegistry {
    /// Loads and validates the `genesis_validators` manifest at `manifest_path`,
    /// zipping it against `assignments` into the ordered [`Validator`] registry.
    ///
    /// Repository-relative paths resolve against the crate root (same rule as
    /// [`ValidatorAssignments::load`]); absolute paths are used verbatim.
    ///
    /// # Errors
    /// - [`DutiesError::YamlRead`] / [`DutiesError::YamlParse`] /
    ///   [`DutiesError::ValidatorsFileTooLarge`] for IO / decode / size failures.
    /// - [`DutiesError::ValidatorPubkeyCountMismatch`] when the manifest length
    ///   does not equal `assignments.total_validators()`.
    /// - [`DutiesError::InvalidValidatorPubkey`] when an entry is not valid
    ///   52-byte hex.
    pub fn load(
        assignments: &ValidatorAssignments,
        manifest_path: impl AsRef<Path>,
    ) -> DutiesResult<Self> {
        let resolved = resolve_path(manifest_path.as_ref());
        let bytes = read_capped(&resolved)?;
        // Reject YAML anchors/aliases BEFORE parsing: alias expansion happens
        // inside `serde_yaml::from_slice` (amplifying a sub-cap file into a huge
        // `Vec`) before any count check can run, so the file-size cap alone does
        // not bound the allocation. A flat hex manifest never needs `&`/`*`.
        if bytes.iter().any(|&b| b == b'&' || b == b'*') {
            return Err(DutiesError::ManifestContainsYamlAlias { path: resolved });
        }
        let raw: RawManifest =
            serde_yaml::from_slice(&bytes).map_err(|source| DutiesError::YamlParse {
                path: resolved,
                source,
            })?;
        Self::from_pubkey_hexes(assignments.total_validators(), &raw.genesis_validators)
    }

    /// Like [`Self::load`], but returns `Ok(None)` when the manifest file does
    /// not exist.
    ///
    /// The absent-vs-present decision is owned here under the SAME path
    /// resolution [`Self::load`] uses (relative paths resolve against the crate
    /// root, not the CWD), so callers must NOT pre-probe the raw path with
    /// `Path::exists` — doing so would resolve against a different root and can
    /// silently disagree with the actual read. Only a genuine `NotFound` is
    /// mapped to `Ok(None)`; every other IO / parse / validation error
    /// propagates.
    ///
    /// # Errors
    /// As for [`Self::load`], except a `NotFound` read error becomes `Ok(None)`
    /// rather than a [`DutiesError::YamlRead`].
    pub fn load_optional(
        assignments: &ValidatorAssignments,
        manifest_path: impl AsRef<Path>,
    ) -> DutiesResult<Option<Self>> {
        match Self::load(assignments, manifest_path) {
            Ok(registry) => Ok(Some(registry)),
            Err(DutiesError::YamlRead { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }

    /// Returns the ordered registry as a borrowed slice (no clone).
    #[must_use]
    pub fn validators(&self) -> &[Validator] {
        &self.validators
    }

    /// Consumes the registry, returning the ordered [`Validator`] buffer by
    /// move (no clone).
    #[must_use]
    pub fn into_validators(self) -> Vec<Validator> {
        self.validators
    }

    /// Pure zip + validation: `expected` validators, one manifest hex each,
    /// decoded to `PublicKey`, paired with the ascending index. The registry is
    /// index-ascending and contiguous `0..expected` by construction.
    fn from_pubkey_hexes(expected: u64, hexes: &[String]) -> DutiesResult<Self> {
        let got = hexes.len() as u64;
        if got != expected {
            return Err(DutiesError::ValidatorPubkeyCountMismatch { expected, got });
        }
        let mut validators = Vec::with_capacity(hexes.len());
        for (position, hex) in hexes.iter().enumerate() {
            let index = position as u64;
            let pubkey = PublicKey::try_from(hex.as_str())
                .map_err(|source| DutiesError::InvalidValidatorPubkey { index, source })?;
            validators.push(Validator {
                pubkey,
                index: ValidatorIndex::new(index),
            });
        }
        Ok(Self { validators })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn hex_pubkey(fill: u8) -> String {
        // Unprefixed lower-case hex, mirroring the manifest's `hex::encode`.
        hex::encode([fill; PublicKey::LEN])
    }

    #[test]
    fn from_pubkey_hexes_loads_index_ordered_registry() {
        let hexes = vec![hex_pubkey(0), hex_pubkey(1), hex_pubkey(2)];
        let reg = GenesisRegistry::from_pubkey_hexes(3, &hexes).unwrap();
        assert_eq!(reg.validators().len(), 3);
        for (i, v) in reg.validators().iter().enumerate() {
            let fill = u8::try_from(i).unwrap();
            assert_eq!(v.index, ValidatorIndex::new(i as u64));
            assert_eq!(v.pubkey.as_slice(), &[fill; PublicKey::LEN]);
        }
    }

    #[test]
    fn from_pubkey_hexes_rejects_count_mismatch() {
        let hexes = vec![hex_pubkey(0), hex_pubkey(1)]; // 2 pubkeys, expected 3
        let err = GenesisRegistry::from_pubkey_hexes(3, &hexes).unwrap_err();
        assert!(
            matches!(
                err,
                DutiesError::ValidatorPubkeyCountMismatch {
                    expected: 3,
                    got: 2
                }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn from_pubkey_hexes_rejects_short_pubkey() {
        let short = "ab".repeat(PublicKey::LEN - 1); // 51 bytes
        let err = GenesisRegistry::from_pubkey_hexes(1, &[short]).unwrap_err();
        assert!(
            matches!(
                err,
                DutiesError::InvalidValidatorPubkey {
                    index: 0,
                    source: types::TypesError::InvalidByteLength {
                        want: 52,
                        got: 51,
                        ..
                    }
                }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn from_pubkey_hexes_rejects_non_hex() {
        let bad = "zz".repeat(PublicKey::LEN);
        let err = GenesisRegistry::from_pubkey_hexes(1, &[bad]).unwrap_err();
        assert!(
            matches!(err, DutiesError::InvalidValidatorPubkey { index: 0, .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn load_rejects_yaml_alias_manifest() {
        // A YAML-alias "bomb" is rejected pre-parse, before serde_yaml can
        // expand it — a flat hex manifest never legitimately contains `&`/`*`.
        let dir = tempfile::tempdir().unwrap();
        let assignments_path = dir.path().join("validators.yaml");
        std::fs::write(&assignments_path, "ream_0:\n  - 0\n").unwrap();
        let assignments = ValidatorAssignments::load(&assignments_path).unwrap();

        let manifest_path = dir.path().join("genesis_validators.yaml");
        // Anchor a scalar and alias it — the shape that amplifies under serde_yaml.
        std::fs::write(&manifest_path, "genesis_validators:\n  - &a 00\n  - *a\n").unwrap();

        let err = GenesisRegistry::load(&assignments, &manifest_path).unwrap_err();
        assert!(
            matches!(err, DutiesError::ManifestContainsYamlAlias { .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn load_zips_manifest_against_assignments() {
        // Full disk round-trip through the PUBLIC APIs only: write both a
        // 2-validator assignment file and its companion manifest, load the
        // assignment via ValidatorAssignments::load (public), then zip via
        // GenesisRegistry::load. Asserts index-ordered pairing — no private
        // constructor, no trivial-pass.
        let dir = tempfile::tempdir().unwrap();

        let assignments_path = dir.path().join("validators.yaml");
        std::fs::write(&assignments_path, "ream_0:\n  - 0\nleanrust_1:\n  - 1\n").unwrap();
        let assignments = ValidatorAssignments::load(&assignments_path).unwrap();
        assert_eq!(assignments.total_validators(), 2);

        let manifest_path = dir.path().join("genesis_validators.yaml");
        std::fs::write(
            &manifest_path,
            format!(
                "genesis_validators:\n  - {}\n  - {}\n",
                hex_pubkey(0),
                hex_pubkey(1)
            ),
        )
        .unwrap();

        let reg = GenesisRegistry::load(&assignments, &manifest_path).unwrap();
        assert_eq!(reg.validators().len(), 2);
        for (i, v) in reg.validators().iter().enumerate() {
            let fill = u8::try_from(i).unwrap();
            assert_eq!(v.index, ValidatorIndex::new(i as u64));
            assert_eq!(v.pubkey.as_slice(), &[fill; PublicKey::LEN]);
        }
    }
}
