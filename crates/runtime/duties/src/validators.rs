//! Validator-assignment YAML loader.
//!
//! Parses the canonical devnet0 shape — a YAML map keyed by group name
//! with a list of validator indices per group:
//!
//! ```yaml
//! ream:       [0, 3, 6, 9, 12, 15, 18, 21, 24, 27]
//! zeam:       [1, 4, 7, 10, 13, 16, 19, 22, 25, 28]
//! quadrivium: [2, 5, 8, 11, 14, 17, 20, 23, 26, 29]
//! ```
//!
//! The alternative `assignments: [{node_name, validators}, ...]` shape
//! used by some upstream fixtures is intentionally out of scope: the
//! devnet0 fixture used here is canonical, and supporting both shapes
//! would expand the loader surface beyond what Issue #30 requires.
//!
//! Loaded assignments are validated end-to-end:
//!
//! - non-empty group map
//! - non-empty per-group validator list
//! - no duplicate validator index across groups
//! - validator indices cover `0..total` contiguously (matches the
//!   upstream `buildAssignments` invariant)

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use protocol::ValidatorIndex;

use super::error::{DutiesError, DutiesResult};

/// Raw YAML shape — group name → list of validator indices.
/// `serde_yaml::from_slice` deserializes directly into this; no
/// wrapper struct needed since the shape is already a stdlib map.
type RawAssignments = BTreeMap<String, Vec<u64>>;

/// Maximum accepted size of a `validators.yaml` file, in bytes.
///
/// devnet0's file is <100 bytes; mainnet validator registries are
/// larger but still well under 1 MiB. The cap bounds the disk read so
/// an operator-supplied (or symlinked) huge file cannot OOM the process
/// before YAML parsing starts — the same `Metadata::len` pattern used
/// for the genesis-SSZ read.
pub(crate) const MAX_VALIDATORS_FILE_BYTES: u64 = 1 << 20;

/// Sentinel path surfaced inside [`DutiesError::YamlParse`] when the
/// parse came from [`ValidatorAssignments::from_bytes`] rather than a
/// real file. Renders as `"<in-memory>"` in error messages.
#[cfg(test)]
const IN_MEMORY_SENTINEL: &str = "<in-memory>";

/// Parsed validator-assignment map: group name → list of validator
/// indices.
///
/// Iteration order of [`Self::groups`] is by group name (the underlying
/// [`BTreeMap`] guarantees deterministic ordering).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorAssignments {
    groups: BTreeMap<String, Vec<ValidatorIndex>>,
    total_validators: u64,
}

impl ValidatorAssignments {
    /// Loads and validates a `validators.yaml` file from disk.
    ///
    /// Repository-relative paths resolve against the `lean-duties`
    /// crate root (`CARGO_MANIFEST_DIR`); absolute paths are used
    /// verbatim.
    ///
    /// # Errors
    /// - [`DutiesError::EmptyValidatorsPath`] for an empty / whitespace
    ///   path.
    /// - [`DutiesError::YamlRead`] when the file cannot be read.
    /// - [`DutiesError::YamlParse`] for YAML decoding failures
    ///   (malformed, non-integer validator entries, etc.).
    /// - [`DutiesError::EmptyAssignmentSet`] /
    ///   [`DutiesError::EmptyValidatorGroupAssignment`] /
    ///   [`DutiesError::DuplicateValidatorAssignment`] /
    ///   [`DutiesError::NonContiguousValidatorSet`] for failed
    ///   semantic-invariant checks.
    pub fn load(path: impl AsRef<Path>) -> DutiesResult<Self> {
        let raw = path.as_ref();
        if raw.as_os_str().is_empty() {
            return Err(DutiesError::EmptyValidatorsPath);
        }
        let resolved = resolve_path(raw);
        // Bound the read at the cap: a capped read rejects an oversized file
        // (including one that grew, or whose symlink target was swapped, after
        // any earlier stat) without slurping unbounded bytes — no check-to-use
        // window. A missing file surfaces uniformly as `YamlRead`.
        let bytes = read_capped(&resolved)?;
        let parsed: RawAssignments =
            serde_yaml::from_slice(&bytes).map_err(|source| DutiesError::YamlParse {
                path: resolved,
                source,
            })?;
        Self::from_canonical(parsed)
    }

    /// Parses a YAML byte slice (no disk read). Test-only entry point
    /// used by the in-file unit tests that exercise loader semantics
    /// without writing a fixture file each time.
    ///
    /// # Errors
    /// As for [`Self::load`] minus the IO branch. Errors carry the
    /// [`IN_MEMORY_SENTINEL`] path so log output explicitly attributes
    /// the failure to an in-memory parse rather than a file.
    #[cfg(test)]
    fn from_bytes(bytes: &[u8]) -> DutiesResult<Self> {
        let parsed: RawAssignments =
            serde_yaml::from_slice(bytes).map_err(|source| DutiesError::YamlParse {
                path: PathBuf::from(IN_MEMORY_SENTINEL),
                source,
            })?;
        Self::from_canonical(parsed)
    }

    /// Returns the validators in `group_name` as a borrowed slice.
    /// Callers that need ownership do `.to_vec()`; the shared-view
    /// shape avoids per-lookup allocation.
    #[must_use]
    pub fn group(&self, group_name: &str) -> Option<&[ValidatorIndex]> {
        self.groups.get(group_name).map(Vec::as_slice)
    }

    /// Returns the total number of validators across every group.
    #[must_use]
    pub const fn total_validators(&self) -> u64 {
        self.total_validators
    }

    fn from_canonical(raw: RawAssignments) -> DutiesResult<Self> {
        if raw.is_empty() {
            return Err(DutiesError::EmptyAssignmentSet);
        }

        // Track first-seen group per validator index for the duplicate
        // diagnostic. `HashMap` because iteration order is never
        // observed — the outer `raw: BTreeMap` provides the
        // deterministic group-visit order.
        let mut seen: HashMap<u64, String> = HashMap::new();
        let mut max_index: u64 = 0;
        let mut total: u64 = 0;
        let mut groups: BTreeMap<String, Vec<ValidatorIndex>> = BTreeMap::new();

        for (name, indices) in raw {
            // A whitespace-only group name is a malformed YAML entry,
            // not an empty assignment set — reuse the per-group variant
            // so the error carries the offending key.
            if name.trim().is_empty() {
                return Err(DutiesError::EmptyValidatorGroupAssignment(name));
            }
            if indices.is_empty() {
                return Err(DutiesError::EmptyValidatorGroupAssignment(name));
            }
            let mut converted = Vec::with_capacity(indices.len());
            for index in indices {
                match seen.entry(index) {
                    Entry::Occupied(existing) => {
                        return Err(DutiesError::DuplicateValidatorAssignment {
                            index,
                            existing_group: existing.get().clone(),
                            conflicting_group: name.clone(),
                        });
                    }
                    Entry::Vacant(slot) => {
                        slot.insert(name.clone());
                    }
                }
                converted.push(ValidatorIndex::new(index));
                if index > max_index {
                    max_index = index;
                }
                // Overflow at 2^64 entries is structurally unreachable
                // — the YAML parser would exhaust memory long before.
                total += 1;
            }
            groups.insert(name, converted);
        }

        // Contiguity: `max_index + 1 == total`. `total == 0` is
        // unreachable (empty group set was caught above).
        if max_index.checked_add(1) != Some(total) {
            return Err(DutiesError::NonContiguousValidatorSet { max_index, total });
        }

        Ok(Self {
            groups,
            total_validators: total,
        })
    }
}

/// Reads `path` into memory, bounding the read at one byte past
/// [`MAX_VALIDATORS_FILE_BYTES`]. Reading that extra byte lets
/// [`check_file_size`] reject an oversized file without ever allocating its
/// full contents, and because the bound is on the read itself (not a prior
/// `Metadata::len` probe) it closes the TOCTOU / symlink-swap window: a file
/// that grows after a stat cannot slurp unbounded memory here.
fn read_capped(path: &Path) -> DutiesResult<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|source| DutiesError::YamlRead {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_VALIDATORS_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DutiesError::YamlRead {
            path: path.to_path_buf(),
            source,
        })?;
    check_file_size(bytes.len() as u64)?;
    Ok(bytes)
}

/// Rejects a file whose byte length exceeds
/// [`MAX_VALIDATORS_FILE_BYTES`]. Extracted so the boundary logic is
/// unit-testable without writing a multi-megabyte fixture to disk.
fn check_file_size(len: u64) -> DutiesResult<()> {
    if len > MAX_VALIDATORS_FILE_BYTES {
        return Err(DutiesError::ValidatorsFileTooLarge {
            size: len,
            cap: MAX_VALIDATORS_FILE_BYTES,
        });
    }
    Ok(())
}

fn resolve_path(raw: &Path) -> PathBuf {
    if raw.is_absolute() {
        return raw.to_path_buf();
    }
    // `CARGO_MANIFEST_DIR` resolves to the crate root at build time,
    // the Rust counterpart to a `runtime.Caller`-based repo-root probe.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(raw)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const CANONICAL_YAML: &str = "\
ream:       [0, 3, 6, 9, 12, 15, 18, 21, 24, 27]
zeam:       [1, 4, 7, 10, 13, 16, 19, 22, 25, 28]
quadrivium: [2, 5, 8, 11, 14, 17, 20, 23, 26, 29]
";

    #[test]
    fn from_bytes_canonical_happy_path() {
        let a = ValidatorAssignments::from_bytes(CANONICAL_YAML.as_bytes()).unwrap();
        assert_eq!(a.total_validators(), 30);
        assert_eq!(
            a.group("ream").unwrap(),
            (0..10)
                .map(|i| ValidatorIndex::new(i * 3))
                .collect::<Vec<_>>()
        );
        assert!(a.group("missing").is_none());
    }

    #[test]
    fn from_bytes_rejects_duplicate_index_across_groups() {
        let yaml = "alpha: [0, 1]\nbeta: [1, 2]\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(
            matches!(
                err,
                DutiesError::DuplicateValidatorAssignment { index: 1, .. }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn from_bytes_rejects_non_contiguous_set() {
        // 4 indices total but max_index = 5 → gap at index 4.
        let yaml = "alpha: [0, 1, 2, 5]\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(
            matches!(
                err,
                DutiesError::NonContiguousValidatorSet {
                    max_index: 5,
                    total: 4
                }
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn from_bytes_rejects_whitespace_only_group_name() {
        // A `"   ": [0, 1]` entry is well-formed YAML but a malformed
        // assignment — surfaces as `EmptyValidatorGroupAssignment` with
        // the offending whitespace key, not `EmptyAssignmentSet`.
        let yaml = "\"   \": [0, 1]\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(
            matches!(err, DutiesError::EmptyValidatorGroupAssignment(ref g) if g.trim().is_empty()),
            "got {err:?}",
        );
    }

    #[test]
    fn from_bytes_rejects_empty_group() {
        let yaml = "alpha: []\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(
            matches!(err, DutiesError::EmptyValidatorGroupAssignment(ref g) if g == "alpha"),
            "got {err:?}",
        );
    }

    #[test]
    fn from_bytes_rejects_empty_assignment_set() {
        let yaml = "{}\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(
            matches!(err, DutiesError::EmptyAssignmentSet),
            "got {err:?}"
        );
    }

    #[test]
    fn from_bytes_rejects_non_integer_validator_entry() {
        let yaml = "alpha: [\"oops\"]\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        assert!(matches!(err, DutiesError::YamlParse { .. }), "got {err:?}");
    }

    #[test]
    fn from_bytes_yaml_parse_error_carries_in_memory_sentinel() {
        let yaml = "alpha: [\"oops\"]\n";
        let err = ValidatorAssignments::from_bytes(yaml.as_bytes()).unwrap_err();
        let display = format!("{err}");
        assert!(
            display.contains(IN_MEMORY_SENTINEL),
            "expected '{IN_MEMORY_SENTINEL}' in error display, got {display}",
        );
    }

    #[test]
    fn check_file_size_accepts_at_and_below_cap() {
        check_file_size(0).unwrap();
        check_file_size(MAX_VALIDATORS_FILE_BYTES).unwrap();
    }

    #[test]
    fn check_file_size_rejects_above_cap() {
        // A 2 MiB file is over the 1 MiB cap.
        let two_mib = 2 * MAX_VALIDATORS_FILE_BYTES;
        let err = check_file_size(two_mib).unwrap_err();
        assert!(
            matches!(
                err,
                DutiesError::ValidatorsFileTooLarge { size, cap }
                    if size == two_mib && cap == MAX_VALIDATORS_FILE_BYTES
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn load_rejects_empty_path() {
        let err = ValidatorAssignments::load("").unwrap_err();
        assert!(
            matches!(err, DutiesError::EmptyValidatorsPath),
            "got {err:?}",
        );
    }

    #[test]
    fn load_surfaces_io_error_for_missing_file() {
        let err = ValidatorAssignments::load("does-not-exist-xyz.yaml").unwrap_err();
        assert!(matches!(err, DutiesError::YamlRead { .. }), "got {err:?}",);
    }
}
