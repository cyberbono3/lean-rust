//! Error type for the duties helpers (config + YAML loader).

use std::path::PathBuf;

use thiserror::Error;

/// Failures raised by the duties helpers.
///
/// The variants below cover config construction (invalid path / group,
/// unset genesis) and the YAML validator-assignment loader. Per-slot
/// production / publish errors are handled by the consensus loop in the
/// `node` crate (warn-and-continue), not here.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DutiesError {
    /// The configured `validators_path` was empty / whitespace-only.
    #[error("duties validators path must not be empty")]
    EmptyValidatorsPath,

    /// The configured `validator_group` selector was empty.
    #[error("duties validator group must not be empty")]
    EmptyValidatorGroup,

    /// The selected validator group was not present in the loaded
    /// assignment file.
    #[error("duties validator group {0:?} not found")]
    UnknownValidatorGroup(String),

    /// The selected validator group exists but has no validators.
    #[error("duties validator group {0:?} assignment is empty")]
    EmptyValidatorGroupAssignment(String),

    /// The loaded assignment file has no validator groups.
    #[error("duties validator assignments are empty")]
    EmptyAssignmentSet,

    /// A validator index appears in more than one group.
    #[error("duties validator {index} assigned to {existing_group:?} and {conflicting_group:?}")]
    DuplicateValidatorAssignment {
        /// Validator index that appears in two groups.
        index: u64,
        /// Group that first claimed the index (encountered earlier in
        /// the sorted YAML map iteration).
        existing_group: String,
        /// Group that re-claimed the index — the source of the
        /// duplicate that produced this error.
        conflicting_group: String,
    },

    /// The union of all groups does not cover a contiguous validator
    /// index range starting from zero.
    #[error("duties validator assignments must be contiguous from zero: max_index={max_index}, total={total}")]
    NonContiguousValidatorSet {
        /// Largest validator index seen across all groups.
        max_index: u64,
        /// Total count of validator indices across all groups.
        total: u64,
    },

    /// `genesis_time_unix` was left at [`super::GenesisTimeUnix::EPOCH`]
    /// (the Unix epoch). Running with epoch genesis makes every slot
    /// fall in the deep past, so the operator sees a "running" node that
    /// schedules fictitious slots. The service refuses to start until a
    /// real genesis time is configured.
    #[error("duties genesis_time_unix must be set (not the Unix epoch)")]
    GenesisTimeUnset,

    /// The validator-assignment file exceeds the configured size cap.
    /// Bounds the read so an operator-supplied (or symlinked) huge file
    /// cannot OOM the process before YAML parsing starts.
    #[error("duties validators file is {size} bytes, exceeds cap of {cap} bytes")]
    ValidatorsFileTooLarge {
        /// Observed file size in bytes.
        size: u64,
        /// Configured maximum in bytes.
        cap: u64,
    },

    /// YAML deserialization failed. Carries the resolved file path the
    /// parse was attempted against — `PathBuf::new()` for test-only
    /// in-memory parses via [`super::ValidatorAssignments::from_bytes`].
    #[error("duties YAML parse error in {path:?}: {source}")]
    YamlParse {
        /// Resolved absolute path that the loader attempted to parse.
        path: PathBuf,
        /// Underlying `serde_yaml` decode error.
        #[source]
        source: serde_yaml::Error,
    },

    /// Reading the validator-assignment file from disk failed.
    /// Verb-symmetric with [`Self::YamlParse`]: parse vs read.
    #[error("duties YAML read {path:?}: {source}")]
    YamlRead {
        /// Resolved absolute path that the loader attempted to read.
        path: PathBuf,
        /// Underlying `io::Error`.
        #[source]
        source: std::io::Error,
    },

    /// The `genesis_validators` manifest pubkey count does not match the
    /// validator total from the assignment file. Never a silent truncation —
    /// every validator index must resolve to exactly one manifest pubkey.
    #[error(
        "genesis_validators manifest has {got} pubkeys, expected {expected} (one per validator)"
    )]
    ValidatorPubkeyCountMismatch {
        /// Validator total from the assignment file (`total_validators`).
        expected: u64,
        /// Number of pubkeys present in the manifest.
        got: u64,
    },

    /// A `genesis_validators` manifest entry failed to decode into a
    /// validator's `PublicKey` (bad hex, or width != 52).
    #[error("genesis_validators pubkey at index {index} is invalid: {source}")]
    InvalidValidatorPubkey {
        /// Zero-based position of the offending manifest entry.
        index: u64,
        /// Underlying `types` decode error (hex or width).
        #[source]
        source: types::TypesError,
    },

    /// The `genesis_validators` manifest contains a YAML anchor (`&`) or alias
    /// (`*`). YAML alias expansion can inflate a small file into an enormous
    /// in-memory collection *during deserialization* — before any entry-count
    /// check can run — defeating the file-size cap and OOM-killing the process.
    /// The manifest is a flat hex sequence that needs neither, so both are
    /// rejected outright.
    #[error("genesis_validators manifest {path:?} must not contain YAML anchors or aliases")]
    ManifestContainsYamlAlias {
        /// Resolved path of the offending manifest.
        path: PathBuf,
    },
}

/// Convenience alias for `Result<T, DutiesError>`. Mirrors the
/// ecosystem-standard `io::Result` / `anyhow::Result` shape so duties
/// signatures stay concise.
pub type DutiesResult<T> = Result<T, DutiesError>;
