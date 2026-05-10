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

pub mod checkpoint;
pub mod error;
pub mod slot;
pub mod validator;

pub use checkpoint::Checkpoint;
pub use error::ProtocolError;
pub use slot::Slot;
pub use validator::{is_proposer, ValidatorIndex};
