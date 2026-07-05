//! Structured outcomes returned by [`super::Engine::import_block`] and
//! [`super::Engine::import_attestation`].
//!
//! Both result types are sum-type enums whose variants carry exactly the
//! fields meaningful to that outcome. This is the idiomatic Rust counterpart
//! to the upstream `(Status, Err)` struct pair: callers pattern-match once,
//! with the compiler enforcing exhaustiveness.

use protocol::ValidatorIndex;
use types::Bytes32;

use super::error::EngineError;

/// Outcome of [`super::Engine::import_block`].
///
/// Variant names mirror the issue-spec ports (`Accepted`, `DuplicateBlock`,
/// `MissingParent`). `Rejected` covers every other failure shape encountered
/// along the state-transition + track path.
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "discarding a block-import outcome silently swallows DuplicateBlock / MissingParent / Rejected"]
pub enum BlockImportResult {
    /// The block transitioned cleanly, was tracked in the store, and the
    /// canonical head was refreshed.
    Accepted {
        /// `hash_tree_root(signed_block.message)`.
        block_root: Bytes32,
        /// `signed_block.message.parent_root`.
        parent_root: Bytes32,
        /// `hash_tree_root(post_state)` after the transition.
        post_state_root: Bytes32,
        /// Canonical head root after the post-import `accept_new_votes` pass.
        head_root: Bytes32,
    },
    /// The block root was already tracked by the store. The call is a no-op.
    DuplicateBlock {
        /// `hash_tree_root(signed_block.message)`.
        block_root: Bytes32,
    },
    /// The block's `parent_root` is not tracked by the store. The store is
    /// left byte-equal to its pre-call state.
    MissingParent {
        /// `hash_tree_root(signed_block.message)`.
        block_root: Bytes32,
        /// `signed_block.message.parent_root`.
        parent_root: Bytes32,
    },
    /// State transition or store invariant rejected the block. The store is
    /// left byte-equal to its pre-call state — [`protocol::State::state_transition`]
    /// is transactional, and the engine performs no mutation before it succeeds.
    Rejected {
        /// `hash_tree_root(signed_block.message)`.
        block_root: Bytes32,
        /// `signed_block.message.parent_root`.
        parent_root: Bytes32,
        /// Underlying failure that triggered the rejection.
        error: EngineError,
    },
}

/// Outcome of [`super::Engine::import_attestation`].
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "discarding an attestation-import outcome silently swallows Ignored / Rejected"]
pub enum AttestationImportResult {
    /// The attestation mutated `latest_new_votes` (gossip pool insert or refresh).
    Accepted {
        /// `signed_vote.validator_id`.
        validator_id: ValidatorIndex,
        /// Store head after the call.
        head_root: Bytes32,
        /// Store safe-target after the call.
        safe_target_root: Bytes32,
    },
    /// The attestation was valid but older or equal to the existing pending
    /// entry for that validator. The store is unchanged.
    Ignored {
        /// `signed_vote.validator_id`.
        validator_id: ValidatorIndex,
        /// Store head after the call.
        head_root: Bytes32,
        /// Store safe-target after the call.
        safe_target_root: Bytes32,
    },
    /// `validate_attestation` or `process_attestation` rejected the vote.
    Rejected {
        /// `signed_vote.validator_id`.
        validator_id: ValidatorIndex,
        /// Underlying failure that triggered the rejection.
        error: EngineError,
    },
}
