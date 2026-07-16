//! Domain consensus types for the Lean Ethereum devnet0 client.
//!
//! Tier 2: depends on [`types`] (foundation) and [`ssz`] (encode / decode /
//! merkleization). No `tracing`, `libp2p`, or runtime imports.
//!
//! # Scope (this revision)
//! - [`Slot`] — `u64` newtype with SSZ codec and the [3SF-mini]
//!   [`Slot::is_justifiable_after`] rule.
//! - [`ValidatorIndex`] — `u64` newtype identifying a registry slot, plus
//!   the round-robin [`is_proposer`] helper.
//! - [`Checkpoint`] — `(root, slot)` container with SSZ codec and Merkle
//!   hash-tree-root.
//! - [`AttestationData`] / [`Attestation`] / [`SignedAttestation`] — the
//!   unsigned vote body, a validator's attestation (its id plus the body), and
//!   the wire-shape container pairing an attestation with its post-quantum
//!   signature.
//! - [`Block`] / [`BlockBody`] / [`BlockHeader`] / [`BlockSignatures`] /
//!   [`BlockWithAttestation`] / [`SignedBlockWithAttestation`] —
//!   variable-length block containers with manual SSZ codecs and Merkle
//!   hash-tree-roots.
//! - [`State`] / [`ProtocolConfig`] — variable-length consensus state
//!   container plus its inner runtime-parameters block, with manual SSZ
//!   codec and Merkle hash-tree-root.
//! - [`ProtocolError`] — crate-level error enum that forwards SSZ failures
//!   from the [`ssz`] facade via `#[from]`.
//!
//! [3SF-mini]: https://github.com/ethereum/consensus-specs
//!
//! # Example
//! ```
//! use protocol::{Checkpoint, ProtocolError, Slot, ValidatorIndex, is_proposer};
//! use ssz::{decode, encode, HashTreeRoot};
//! use types::Bytes32;
//!
//! # fn main() -> Result<(), ProtocolError> {
//! assert!(Slot::new(9).is_justifiable_after(Slot::new(0)));   // δ = 9 = 3²
//! assert!(!Slot::new(7).is_justifiable_after(Slot::new(0)));  // δ = 7 — neither
//!
//! assert!(is_proposer(ValidatorIndex::new(2), Slot::new(2), 4)?);
//!
//! let cp = Checkpoint::new(Bytes32::zero(), Slot::new(9));
//! let bytes = encode(&cp);
//! let back: Checkpoint = decode(&bytes)?; // SszError → ProtocolError::Ssz
//! assert_eq!(back, cp);
//! let _root: [u8; 32] = cp.hash_tree_root();
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod internal;
// Cross-client genesis interop: holds the decoder for the compact
// "leanchain" state SSZ shared with the ream client. Private module — it
// only adds the inherent `State::from_ream_legacy_ssz_bytes` method, which
// stays reachable on `State` regardless of module visibility.
mod ream;
#[cfg(test)]
mod test_fixtures;

pub mod block;
pub mod checkpoint;
pub mod error;
pub mod slot;
pub mod state;
pub mod stf;
pub mod validator;
pub mod vote;

pub use block::{
    Block, BlockBody, BlockHeader, BlockSignatures, BlockWithAttestation,
    SignedBlockWithAttestation, MAX_ATTESTATIONS,
};
pub use checkpoint::Checkpoint;
pub use error::{AttSlotKind, ProtocolError, StateTransitionError};
pub use slot::Slot;
pub use state::{
    ProtocolConfig, State, HISTORICAL_ROOTS_LIMIT, JUSTIFICATIONS_VALIDATORS_LIMIT,
    STATE_FIXED_PART_LEN, VALIDATOR_REGISTRY_LIMIT,
};
pub use validator::{is_proposer, ValidatorIndex};
pub use vote::{Attestation, AttestationData, SignedAttestation};
