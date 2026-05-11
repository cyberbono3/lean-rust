//! Forkchoice store: blocks, post-states, votes, and the canonical clock.
//!
//! [`Store`] tracks every block + post-state pair received by the node, plus
//! the validator vote maps used by LMD-GHOST. This revision exposes only the
//! data container, the `from_anchor` constructor, and minimal read accessors.
//! Block-add, attestation processing, and head resolution land in subsequent
//! issues in this part.
//!
//! # `block_order` invariant
//!
//! `block_order` records the order in which roots first entered `blocks`.
//! Re-inserting a known root is a no-op for both the maps and the order
//! vector. The vector is **first-seen order, not slot-sorted** — the
//! canonical tie-break for head resolution operates on `(weight, slot, root)`
//! and never consults `block_order`.
//!
//! # Vote-map cost note
//!
//! `latest_known_votes` and `latest_new_votes` hold [`SignedVote`] values
//! by-value. Each [`SignedVote`] carries a 4000-byte signature placeholder,
//! so the maps grow by ≈4 KB per validator. The maps stay empty in this
//! revision; population begins when attestation processing lands.

use std::collections::HashMap;

use config::INTERVALS_PER_SLOT;
use protocol::{Block, Checkpoint, ProtocolConfig, SignedVote, State, ValidatorIndex};
use ssz::HashTreeRoot;
use types::Bytes32;

use crate::error::ForkchoiceError;

/// Forkchoice clock value: intervals since genesis.
///
/// Aliased rather than newtyped while the surface stays small. The newtype
/// upgrade is anticipated when `tick_interval` lands and benefits from
/// saturating arithmetic.
pub type Time = u64;

/// LMD-GHOST forkchoice store.
///
/// Mirrors the data shape of upstream `consensus/forkchoice.Store` while
/// staying idiomatic to Rust (private fields, `&`-returning accessors, no
/// `defer FuncTrace`). All inputs are owned; [`Self::from_anchor`] takes
/// `state` and `anchor_block` by value to avoid the implicit clone they
/// would require to enter the maps.
///
/// # `Default`
///
/// [`Store::default()`] returns an all-zero structural placeholder: empty
/// maps, zero-root `head` and `safe_target`, zero-slot `latest_justified` /
/// `latest_finalized`, and `time = 0`. It is **not** a usable fork-choice
/// state — every accessor that resolves a root will return `None` because
/// the zero root is not tracked. Use [`Self::from_anchor`] for real
/// construction. The `Default` impl exists for struct-update syntax,
/// `mem::take`, and test scaffolding.
#[derive(Debug, Clone, Default)]
pub struct Store {
    time: Time,
    config: ProtocolConfig,
    head: Bytes32,
    safe_target: Bytes32,
    latest_justified: Checkpoint,
    latest_finalized: Checkpoint,
    blocks: HashMap<Bytes32, Block>,
    states: HashMap<Bytes32, State>,
    block_order: Vec<Bytes32>,
    latest_known_votes: HashMap<ValidatorIndex, SignedVote>,
    latest_new_votes: HashMap<ValidatorIndex, SignedVote>,
}

impl Store {
    /// Builds a fresh forkchoice store from a trusted anchor pair.
    ///
    /// `anchor_block.state_root` must equal `hash_tree_root(state)`. The
    /// anchor's clock value is `anchor_block.slot * INTERVALS_PER_SLOT`. On
    /// success `state` and `anchor_block` are moved into the store; the
    /// `block_order` vector is seeded with the single anchor root, and both
    /// `head` and `safe_target` point at the anchor.
    ///
    /// # Errors
    /// - [`ForkchoiceError::AnchorStateRootMismatch`] when
    ///   `anchor_block.state_root != state.hash_tree_root()`. Both inputs
    ///   are dropped.
    /// - [`ForkchoiceError::AnchorTimeOverflow`] when
    ///   `anchor_block.slot * INTERVALS_PER_SLOT` overflows `u64`.
    pub fn from_anchor(state: State, anchor_block: Block) -> Result<Self, ForkchoiceError> {
        // 1. State-root parity check (cheap; runs before any move).
        let want_state_root: Bytes32 = state.hash_tree_root().into();
        if anchor_block.state_root != want_state_root {
            return Err(ForkchoiceError::AnchorStateRootMismatch {
                got: anchor_block.state_root,
                want: want_state_root,
            });
        }

        // 2. Derived values.
        let anchor_root: Bytes32 = anchor_block.hash_tree_root().into();
        let time = anchor_block
            .slot
            .get()
            .checked_mul(INTERVALS_PER_SLOT)
            .ok_or(ForkchoiceError::AnchorTimeOverflow {
                slot: anchor_block.slot,
                intervals_per_slot: INTERVALS_PER_SLOT,
            })?;

        // 3. Build the store with the seeded fields; remaining fields
        //    (maps, block_order, vote maps) take their `Default` empty
        //    values. `state.config` / `latest_justified` / `latest_finalized`
        //    are `Copy`, so they are read inline before `state` moves into
        //    `insert_block`.
        let mut store = Self {
            time,
            config: state.config,
            head: anchor_root,
            safe_target: anchor_root,
            latest_justified: state.latest_justified,
            latest_finalized: state.latest_finalized,
            ..Default::default()
        };
        store.insert_block(anchor_root, anchor_block, state);
        Ok(store)
    }

    /// Returns the store's current forkchoice time (intervals since genesis).
    #[must_use]
    pub fn time(&self) -> Time {
        self.time
    }

    /// Returns the in-state runtime parameters carried alongside the chain.
    #[must_use]
    pub fn config(&self) -> &ProtocolConfig {
        &self.config
    }

    /// Returns the current canonical head root.
    #[must_use]
    pub fn head(&self) -> Bytes32 {
        self.head
    }

    /// Returns the current safe attestation target root.
    #[must_use]
    pub fn safe_target(&self) -> Bytes32 {
        self.safe_target
    }

    /// Returns the highest justified checkpoint known to the store.
    #[must_use]
    pub fn latest_justified(&self) -> Checkpoint {
        self.latest_justified
    }

    /// Returns the highest finalized checkpoint known to the store.
    #[must_use]
    pub fn latest_finalized(&self) -> Checkpoint {
        self.latest_finalized
    }

    /// Returns the block-insertion order (first-seen order, no duplicates).
    #[must_use]
    pub fn block_order(&self) -> &[Bytes32] {
        &self.block_order
    }

    /// Resolves a tracked block by root. Returns `None` if the root is
    /// unknown to the store.
    #[must_use]
    pub fn block(&self, root: &Bytes32) -> Option<&Block> {
        self.blocks.get(root)
    }

    /// Resolves a tracked post-state by block root. Returns `None` if the
    /// root is unknown to the store.
    #[must_use]
    pub fn state(&self, root: &Bytes32) -> Option<&State> {
        self.states.get(root)
    }

    /// Reports whether `root` is known to the store.
    #[must_use]
    pub fn has_block(&self, root: &Bytes32) -> bool {
        self.blocks.contains_key(root)
    }

    /// Returns the accepted full [`SignedVote`]s by validator. Empty in this
    /// revision; population begins when attestation processing lands.
    #[must_use]
    pub fn latest_known_votes(&self) -> &HashMap<ValidatorIndex, SignedVote> {
        &self.latest_known_votes
    }

    /// Returns pending full [`SignedVote`]s received via gossip but not yet
    /// promoted into [`Self::latest_known_votes`]. Empty in this revision.
    #[must_use]
    pub fn latest_new_votes(&self) -> &HashMap<ValidatorIndex, SignedVote> {
        &self.latest_new_votes
    }

    /// Internal: invariant-preserving block-insert.
    ///
    /// On the first insert of `root`, both maps and `block_order` gain the
    /// triple. On any re-insert the call is a no-op (no duplicate in
    /// `block_order`). Public block-add lands in the next forkchoice issue;
    /// this method exists so the anchor seed and the `block_order` invariant
    /// test can share one code path.
    pub(crate) fn insert_block(&mut self, root: Bytes32, block: Block, state: State) {
        if self.blocks.insert(root, block).is_none() {
            self.states.insert(root, state);
            self.block_order.push(root);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::Slot;
    use static_assertions::assert_impl_all;
    use std::hash::Hash;
    use types::Bytes32;

    use crate::test_fixtures::{anchor_pair, anchor_pair_at_slot};

    // -- Compile-time witness ----------------------------------------------

    assert_impl_all!(Bytes32: Hash, Eq, PartialEq);

    // -- Validation: rejection paths ---------------------------------------

    #[test]
    fn from_anchor_rejects_state_root_mismatch() {
        let (state, mut block) = anchor_pair(4);
        let want: Bytes32 = state.hash_tree_root().into();
        block.state_root = Bytes32::new([0xff; 32]);
        let err = Store::from_anchor(state, block).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::AnchorStateRootMismatch {
                got: Bytes32::new([0xff; 32]),
                want,
            }
        );
    }

    #[test]
    fn from_anchor_rejects_when_slot_intervals_overflow_u64() {
        let overflow_slot = Slot::new(u64::MAX / INTERVALS_PER_SLOT + 1);
        let (state, block) = anchor_pair_at_slot(overflow_slot, 4);
        let err = Store::from_anchor(state, block).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::AnchorTimeOverflow {
                slot: overflow_slot,
                intervals_per_slot: INTERVALS_PER_SLOT,
            }
        );
    }

    // -- Construction: head + safe target + clock --------------------------

    #[test]
    fn from_anchor_seeds_head_and_safe_target_to_anchor_root() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.head(), anchor_root);
        assert_eq!(store.safe_target(), anchor_root);
    }

    #[test]
    fn from_anchor_seeds_time_at_slot_zero() {
        let (state, block) = anchor_pair(4);
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.time(), 0);
    }

    #[test]
    fn from_anchor_seeds_time_at_non_zero_slot() {
        let (state, block) = anchor_pair_at_slot(Slot::new(5), 4);
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.time(), 5 * INTERVALS_PER_SLOT);
    }

    // -- Construction: maps + block_order ----------------------------------

    #[test]
    fn from_anchor_seeds_block_and_state_maps() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let pre_state_root: Bytes32 = state.hash_tree_root().into();
        let store = Store::from_anchor(state, block.clone()).unwrap();
        assert_eq!(store.block(&anchor_root), Some(&block));
        let stored_state_root: Bytes32 = store.state(&anchor_root).unwrap().hash_tree_root().into();
        assert_eq!(stored_state_root, pre_state_root);
    }

    #[test]
    fn from_anchor_seeds_block_order_singleton() {
        let (state, block) = anchor_pair(4);
        let anchor_root: Bytes32 = block.hash_tree_root().into();
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.block_order(), &[anchor_root]);
        assert!(store.has_block(&anchor_root));
    }

    // -- Construction: checkpoint inheritance ------------------------------

    #[test]
    fn from_anchor_inherits_justified_finalized_from_state() {
        let (state, block) = anchor_pair(4);
        let want_justified = state.latest_justified;
        let want_finalized = state.latest_finalized;
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.latest_justified(), want_justified);
        assert_eq!(store.latest_finalized(), want_finalized);
    }

    #[test]
    fn from_anchor_empty_vote_maps() {
        let (state, block) = anchor_pair(4);
        let store = Store::from_anchor(state, block).unwrap();
        assert!(store.latest_known_votes().is_empty());
        assert!(store.latest_new_votes().is_empty());
    }

    // -- Default sentinel --------------------------------------------------

    #[test]
    fn default_is_structural_placeholder() {
        let store = Store::default();
        assert_eq!(store.time(), 0);
        assert_eq!(store.head(), Bytes32::zero());
        assert_eq!(store.safe_target(), Bytes32::zero());
        assert!(store.block_order().is_empty());
        assert!(store.latest_known_votes().is_empty());
        assert!(store.latest_new_votes().is_empty());
        // Zero root is not tracked — accessors must return None.
        assert!(store.block(&Bytes32::zero()).is_none());
        assert!(store.state(&Bytes32::zero()).is_none());
        assert!(!store.has_block(&Bytes32::zero()));
    }

    // -- block_order invariant ---------------------------------------------

    #[test]
    fn block_order_first_seen_invariant_preserved_by_internal_insert() {
        let (state_a, block_a) = anchor_pair(4);
        let anchor_root_a: Bytes32 = block_a.hash_tree_root().into();
        let mut store = Store::from_anchor(state_a, block_a).unwrap();

        // Insert a second distinct (root, block, state) triple.
        let (state_b, block_b) = anchor_pair_at_slot(Slot::new(5), 4);
        let root_b: Bytes32 = block_b.hash_tree_root().into();
        store.insert_block(root_b, block_b.clone(), state_b.clone());
        assert_eq!(store.block_order(), &[anchor_root_a, root_b]);

        // Re-insert the second root with the same payload — must be a no-op.
        store.insert_block(root_b, block_b, state_b);
        assert_eq!(store.block_order(), &[anchor_root_a, root_b]);
    }
}
