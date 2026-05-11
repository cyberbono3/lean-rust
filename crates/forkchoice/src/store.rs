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
use crate::time::{Phase, Time};

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
            .map(Time::new)
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

    // ==================================================================
    // tick_interval — 4-phase clock advance
    // ==================================================================

    /// Slot index derived from the current clock value.
    #[must_use]
    pub fn current_slot(&self) -> u64 {
        self.time.slot()
    }

    /// Intra-slot interval index derived from the current clock value.
    #[must_use]
    pub fn current_interval(&self) -> u64 {
        self.time.interval()
    }

    /// Spec [`Phase`] of the current clock value.
    pub fn current_phase(&self) -> Phase {
        self.time.phase()
    }

    /// Advances the forkchoice clock by one interval and dispatches the
    /// phase-specific hook based on the *new* time.
    ///
    /// The four phases are mandated by leanSpec; see [`Phase`] for the
    /// variant-to-hook mapping. Exhaustive matching on [`Phase`] guards
    /// future spec changes against silent reordering.
    ///
    /// # Errors
    /// - [`ForkchoiceError::TimeOverflow`] when `self.time().get() == u64::MAX`.
    /// - Forwarded from [`Self::accept_new_votes`] /
    ///   [`Self::update_safe_target`] when their hook bodies land in #18.
    pub fn tick_interval(&mut self, has_proposal: bool) -> Result<(), ForkchoiceError> {
        let next = self
            .time
            .checked_advance()
            .ok_or(ForkchoiceError::TimeOverflow { time: self.time })?;
        self.time = next;

        match next.phase() {
            Phase::Proposal if has_proposal => self.accept_new_votes(),
            Phase::Proposal | Phase::Idle => Ok(()),
            Phase::UpdateSafeTarget => self.update_safe_target(),
            Phase::AcceptNewVotes => self.accept_new_votes(),
        }
    }

    // -- Phase hook stubs (bodies land in #18). -------------------------

    /// Promotes pending votes into the known vote set and refreshes the
    /// forkchoice head. **Stub:** body lands in #18; this revision returns
    /// `Ok(())` so [`Self::tick_interval`]'s dispatch shape is testable.
    ///
    /// # Errors
    /// Currently infallible. The `Result` return matches the upstream
    /// signature and stays forward-compatible for the #18 body.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn accept_new_votes(&mut self) -> Result<(), ForkchoiceError> {
        // TODO(#18): promote latest_new_votes into latest_known_votes,
        //            then refresh head via LMD-GHOST.
        Ok(())
    }

    /// Recomputes the safe attestation target using the supermajority
    /// filter. **Stub:** body lands in #18; this revision returns `Ok(())`.
    ///
    /// # Errors
    /// Currently infallible. The `Result` return matches the upstream
    /// signature and stays forward-compatible for the #18 body.
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn update_safe_target(&mut self) -> Result<(), ForkchoiceError> {
        // TODO(#18): recompute safe_target using the supermajority filter.
        Ok(())
    }

    /// Test-only builder that overrides the constructor-seeded time.
    /// Lets table-driven and property tests position the clock without
    /// running `tick_interval` from genesis.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_time_for_test(mut self, time: Time) -> Self {
        self.time = time;
        self
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
        assert_eq!(store.time(), Time::ZERO);
    }

    #[test]
    fn from_anchor_seeds_time_at_non_zero_slot() {
        let (state, block) = anchor_pair_at_slot(Slot::new(5), 4);
        let store = Store::from_anchor(state, block).unwrap();
        assert_eq!(store.time(), Time::new(5 * INTERVALS_PER_SLOT));
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
        assert_eq!(store.time(), Time::ZERO);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tick_interval_tests {
    use super::*;
    use proptest::prelude::*;

    use crate::test_fixtures::anchor_pair;

    /// Builds an anchored store and positions the clock at `start` via the
    /// test-only builder.
    fn store_at(start: Time) -> Store {
        let (state, block) = anchor_pair(4);
        Store::from_anchor(state, block)
            .unwrap()
            .with_time_for_test(start)
    }

    // -- Table-driven dispatch -------------------------------------------

    #[test]
    fn tick_advances_one_interval_and_resolves_phase() {
        // (start_time, has_proposal, end_time, end_phase)
        let cases: &[(u64, bool, u64, Phase)] = &[
            // Slot 0 intervals 0..3.
            (0, false, 1, Phase::Idle),
            (1, false, 2, Phase::UpdateSafeTarget),
            (2, false, 3, Phase::AcceptNewVotes),
            // Slot rollover: interval 3 → interval 0 of next slot.
            (3, false, 4, Phase::Proposal),
            (3, true, 4, Phase::Proposal),
            // Further slot rollovers.
            (7, false, 8, Phase::Proposal),
            (7, true, 8, Phase::Proposal),
            (11, true, 12, Phase::Proposal),
            // Within-slot transitions at high time.
            (1_000, false, 1_001, Phase::Idle),
            (1_001, false, 1_002, Phase::UpdateSafeTarget),
            (1_002, false, 1_003, Phase::AcceptNewVotes),
            (1_003, true, 1_004, Phase::Proposal),
        ];
        for &(start, has_proposal, end, phase) in cases {
            let mut store = store_at(Time::new(start));
            store.tick_interval(has_proposal).unwrap_or_else(|e| {
                panic!("tick_interval({has_proposal}) failed at start {start}: {e}")
            });
            assert_eq!(
                store.time(),
                Time::new(end),
                "time after tick from {start} with has_proposal={has_proposal}"
            );
            assert_eq!(store.current_phase(), phase, "phase at {end}");
            assert_eq!(
                store.current_slot(),
                end / INTERVALS_PER_SLOT,
                "slot at {end}"
            );
            assert_eq!(
                store.current_interval(),
                end % INTERVALS_PER_SLOT,
                "interval at {end}"
            );
        }
    }

    // -- Error path ------------------------------------------------------

    #[test]
    fn tick_rejects_time_overflow() {
        let mut store = store_at(Time::new(u64::MAX));
        let err = store.tick_interval(false).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::TimeOverflow {
                time: Time::new(u64::MAX)
            }
        );
    }

    #[test]
    fn overflow_error_path_leaves_state_unchanged() {
        let mut store = store_at(Time::new(u64::MAX));
        let snapshot_time = store.time();
        let _ = store.tick_interval(true).unwrap_err();
        assert_eq!(store.time(), snapshot_time);
    }

    // -- Property tests --------------------------------------------------

    proptest! {
        /// Each `tick_interval` advances `time` by exactly 1 and resolves
        /// `current_phase()` to `Time::new(new_time).phase()`.
        #[test]
        fn tick_state_machine_is_plus_one_with_phase_from_time(
            start in 0_u64..(u64::MAX - 256),
            steps in proptest::collection::vec(any::<bool>(), 1..=64),
        ) {
            let mut store = store_at(Time::new(start));
            for &has_proposal in &steps {
                let before = store.time().get();
                store.tick_interval(has_proposal).unwrap();
                let after = store.time().get();
                prop_assert_eq!(after, before + 1);
                prop_assert_eq!(store.current_phase(), Time::new(after).phase());
            }
        }

        /// Phase classification is periodic with period `INTERVALS_PER_SLOT`.
        #[test]
        fn phase_is_periodic_modulo_intervals_per_slot(
            t in 0_u64..(u64::MAX - INTERVALS_PER_SLOT),
        ) {
            prop_assert_eq!(
                Time::new(t).phase(),
                Time::new(t + INTERVALS_PER_SLOT).phase()
            );
        }
    }
}
