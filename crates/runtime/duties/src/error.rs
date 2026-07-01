//! Error type for the duties [`Service`](super::Service).

use std::path::PathBuf;

use thiserror::Error;

use lean_chain::ChainError;

use super::publisher::PublishError;

/// Failures raised by the duties service.
///
/// Per-slot production / publish errors are *not* terminal: they are
/// warn-logged and folded into the scheduler's publish-health counter
/// (see [`super::Service`]). The variants
/// below cover construction (invalid config, missing group) and the YAML
/// loader.
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

    /// A `slot_duration_ms` of zero was supplied to a config builder.
    /// Modelled as a [`core::num::NonZeroU64`] at the type level so the
    /// divide-by-zero in the scheduler's slot math is unreachable; this
    /// variant surfaces the rejection at the loose-input boundary.
    #[error("duties slot_duration_ms must be non-zero")]
    ZeroSlotDuration,

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

    /// A per-validator attestation duty (produce + publish) did not
    /// complete within its slot budget and was cancelled by the
    /// per-future timeout. Recorded in publish health like any other
    /// duty failure.
    #[error("duties attestation for validator {validator} timed out after {timeout_ms} ms")]
    DutyTimeout {
        /// Validator whose duty timed out.
        validator: u64,
        /// Per-validator budget in milliseconds.
        timeout_ms: u64,
    },

    /// `Service::start` was called twice.
    #[error("duties service already started")]
    AlreadyStarted,

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

    /// Chain production / persistence path failed during a scheduled
    /// duty. Surfaces only via `Service::status()` (the scheduler does
    /// not terminate on a single failure). Display is forwarded
    /// verbatim from the inner [`ChainError`] so log output reads
    /// `"storage: ..."` / `"engine: ..."` without a redundant prefix.
    #[error(transparent)]
    Chain(#[from] ChainError),

    /// Publishing a produced block / attestation failed. Recorded in
    /// the scheduler's [`super::Service`] publish-health counter rather
    /// than terminating the worker — a single transport flake is not a
    /// service-terminal condition. Display forwards the inner
    /// [`PublishError`] verbatim.
    #[error(transparent)]
    Publish(#[from] PublishError),
}

/// Convenience alias for `Result<T, DutiesError>`. Mirrors the
/// ecosystem-standard `io::Result` / `anyhow::Result` shape so duties
/// signatures stay concise.
pub type DutiesResult<T> = Result<T, DutiesError>;
