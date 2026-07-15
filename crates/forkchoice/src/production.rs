//! Local block + attestation production driven by the forkchoice store.
//!
//! Mirrors leanSpec `forkchoice/production.py` and the upstream
//! forkchoice production shape:
//!
//! - [`Store::produce_block`] runs the propose-and-track flow: derive the
//!   proposal head, build a candidate, gather includable votes from
//!   `latest_known_votes` (filtered by source justification and target
//!   tracking), repeat until the vote set stabilizes, then track the
//!   block in the store.
//! - [`Store::produce_attestation_vote`] composes the local attestation
//!   `Vote` from `(head, target, source)` plus the store's current
//!   safe-target snapshot.
//!
//! Both flows are pure with respect to networking/runtime — this module
//! has no I/O, no async, and no dependencies outside the state-transition
//! and protocol crates.

use std::cmp::Ordering;

use protocol::{
    is_proposer, Block, BlockBody, Checkpoint, SignedVote, Slot, State, ValidatorIndex, Vote,
    MAX_ATTESTATIONS,
};
use ssz::HashTreeRoot;
use types::Bytes32;

use crate::error::ForkchoiceError;
use crate::store::Store;

/// Structured result of [`Store::produce_block`].
///
/// `root` and `post_state_root` are cached hashes — callers may recompute
/// them via `block.hash_tree_root()` / `post_state.hash_tree_root()`.
#[derive(Debug, Clone)]
pub struct ProducedBlock {
    /// The freshly built block with `state_root` set to
    /// `hash_tree_root(post_state)`.
    pub block: Block,
    /// `hash_tree_root(block)` captured at production time.
    pub root: Bytes32,
    /// The parent root from which production was anchored — equal to
    /// [`Self::block`]`.parent_root`.
    pub parent_root: Bytes32,
    /// Post-state produced by the candidate's state transition.
    pub post_state: State,
    /// `hash_tree_root(post_state)` captured at production time.
    pub post_state_root: Bytes32,
}

/// Structured result of [`Store::produce_attestation_vote`].
///
/// Every field is `Copy`, so the whole struct derives `Copy` and avoids
/// clones at call sites.
#[derive(Debug, Clone, Copy)]
pub struct ProducedVote {
    /// The unsigned vote ready for signing by the validator client.
    pub vote: Vote,
    /// `vote.head.root`, surfaced separately for log/metric convenience.
    pub head_root: Bytes32,
    /// `vote.target`, surfaced separately.
    pub target: Checkpoint,
    /// `vote.source`, surfaced separately.
    pub source: Checkpoint,
    /// Snapshot of the store's `safe_target` at production time.
    pub safe_target: Bytes32,
}

impl Store {
    /// Produces a local unsigned block for `slot`, proposed by
    /// `validator`. The flow:
    ///
    /// 1. Authorize: `validator` must be the round-robin proposer for
    ///    `slot`.
    /// 2. Resolve proposal head and pull out the head's post-state.
    /// 3. Loop:
    ///    - Build a candidate block + post-state from the current
    ///      attestation set.
    ///    - Filter `latest_known_votes` by `(target tracked, source ==
    ///      post_state.latest_justified, not-already-included)`.
    ///    - If no new votes are includable, finalize: write the
    ///      `state_root` and track the block in the store.
    ///    - Otherwise append the new votes and repeat.
    ///
    /// The loop terminates: the includable set is monotonically growing
    /// and bounded by `min(MAX_ATTESTATIONS, latest_known_votes.len())`.
    ///
    /// # Errors
    /// - [`ForkchoiceError::UnauthorizedProposer`] when `validator` is not
    ///   the round-robin proposer for `slot`.
    /// - [`ForkchoiceError::HeadStateNotFound`] when the proposal head's
    ///   post-state is missing.
    /// - Forwarded from [`Self::get_proposal_head`],
    ///   [`State::process_slots`], [`State::process_block_header`],
    ///   [`State::process_attestations`], and [`Self::track_block`].
    pub fn produce_block(
        &mut self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<ProducedBlock, ForkchoiceError> {
        if !is_proposer(validator, slot, self.config().num_validators)? {
            return Err(ForkchoiceError::UnauthorizedProposer { validator, slot });
        }

        let head_root = self.get_proposal_head()?;
        let head_state = self
            .state(&head_root)
            .cloned()
            .ok_or(ForkchoiceError::HeadStateNotFound { root: head_root })?;

        let (attestations, post_state) =
            self.converge_attestations(head_root, slot, validator, &head_state)?;
        self.finalize_produced_block(head_root, slot, validator, attestations, post_state)
    }

    /// Convergent attestation gathering: rebuild the candidate's post-state
    /// against the current attestation set, fold in any newly includable
    /// votes, repeat until the includable set is empty. Returns the stable
    /// `(attestations, post_state)` pair.
    ///
    /// Termination is guaranteed: `collect_includable_votes` only ever
    /// returns votes not already in `attestations`, and the result is
    /// bounded by `min(MAX_ATTESTATIONS, latest_known_votes.len())`.
    fn converge_attestations(
        &self,
        head_root: Bytes32,
        slot: Slot,
        validator: ValidatorIndex,
        head_state: &State,
    ) -> Result<(Vec<SignedVote>, State), ForkchoiceError> {
        let mut attestations: Vec<SignedVote> = Vec::new();
        loop {
            let (_, post_state) =
                build_candidate_block(head_root, slot, validator, head_state, &attestations)?;
            let mut additions = self.collect_includable_votes(&post_state, &attestations);
            if additions.is_empty() {
                return Ok((attestations, post_state));
            }
            attestations.append(&mut additions);
        }
    }

    /// Builds the final block from the stabilized `(attestations,
    /// post_state)` pair, tracks the result in the store, and refreshes
    /// forkchoice head against the expanded block tree.
    fn finalize_produced_block(
        &mut self,
        head_root: Bytes32,
        slot: Slot,
        validator: ValidatorIndex,
        attestations: Vec<SignedVote>,
        post_state: State,
    ) -> Result<ProducedBlock, ForkchoiceError> {
        let post_state_root: Bytes32 = post_state.hash_tree_root().into();
        let block = Block {
            slot,
            proposer_index: validator,
            parent_root: head_root,
            state_root: post_state_root,
            body: BlockBody { attestations },
        };
        let root: Bytes32 = block.hash_tree_root().into();
        self.track_block(block.clone(), post_state.clone())?;
        self.accept_new_votes()?;
        Ok(ProducedBlock {
            block,
            root,
            parent_root: head_root,
            post_state,
            post_state_root,
        })
    }

    /// Produces a local unsigned attestation vote for `slot`.
    ///
    /// `head` is the current proposal head; `target` is derived by
    /// [`Self::get_vote_target`] (at most three hops back from `head`);
    /// `source` is the store's `latest_justified` checkpoint.
    ///
    /// # Errors
    /// - Forwarded from [`Self::get_proposal_head`].
    /// - [`ForkchoiceError::UnknownHeadBlock`] when the resolved head root
    ///   is absent from the block map.
    /// - Forwarded from [`Self::get_vote_target`].
    pub fn produce_attestation_vote(
        &mut self,
        slot: Slot,
    ) -> Result<ProducedVote, ForkchoiceError> {
        let head_root = self.get_proposal_head()?;
        let head_slot = self
            .block(&head_root)
            .ok_or(ForkchoiceError::UnknownHeadBlock { root: head_root })?
            .slot;
        let target = self.get_vote_target()?;
        let source = self.latest_justified();

        let vote = Vote {
            slot,
            head: Checkpoint::new(head_root, head_slot),
            target,
            source,
        };
        Ok(ProducedVote {
            vote,
            head_root,
            target,
            source,
            safe_target: self.safe_target(),
        })
    }

    /// Filters `latest_known_votes` for votes the producer can include in
    /// the candidate block: (1) the vote's `target.root` must be tracked,
    /// (2) the vote's `source` must equal the candidate post-state's
    /// `latest_justified` (matches ream's `propose_block` pre-filter),
    /// (3) the vote must not already appear in `already_included`.
    ///
    /// The result is capped at `MAX_ATTESTATIONS - already_included.len()`
    /// so the candidate block never exceeds the SSZ list bound.
    fn collect_includable_votes(
        &self,
        post_state: &State,
        already_included: &[SignedVote],
    ) -> Vec<SignedVote> {
        let cap = MAX_ATTESTATIONS.saturating_sub(already_included.len());
        if cap == 0 {
            return Vec::new();
        }
        self.latest_known_votes()
            .values()
            .filter(|sv| self.has_block(&sv.message.target.root))
            .filter(|sv| sv.message.source == post_state.latest_justified)
            .filter(|sv| !already_included.contains(sv))
            .take(cap)
            .cloned()
            .collect()
    }
}

/// Builds a candidate block + post-state for `(slot, validator)` over
/// `votes`. The candidate's `state_root` is left as `Bytes32::zero()`;
/// [`Store::produce_block`] commits the real value once the convergence
/// loop terminates.
fn build_candidate_block(
    head_root: Bytes32,
    slot: Slot,
    validator: ValidatorIndex,
    head_state: &State,
    votes: &[SignedVote],
) -> Result<(Block, State), ForkchoiceError> {
    let candidate = Block {
        slot,
        proposer_index: validator,
        parent_root: head_root,
        state_root: Bytes32::zero(),
        body: BlockBody {
            attestations: votes.to_vec(),
        },
    };

    let mut next = advance_state_to_slot(head_state.clone(), slot)?;
    next.process_block_header(&candidate)?;
    next.process_attestations(&candidate.body.attestations)?;
    Ok((candidate, next))
}

/// Advances `state` to `target` via `process_slots`. A no-op when the
/// state is already at the target slot. Rejects backwards travel.
fn advance_state_to_slot(mut state: State, target: Slot) -> Result<State, ForkchoiceError> {
    match state.slot.cmp(&target) {
        Ordering::Equal => Ok(state),
        Ordering::Less => {
            state.process_slots(target)?;
            Ok(state)
        }
        Ordering::Greater => Err(ForkchoiceError::StateSlotAheadOfTarget {
            state_slot: state.slot,
            target_slot: target,
        }),
    }
}

// Fixtures here still build the deprecated `Bytes4000` placeholder. `expect`
// rather than `allow` so it retires itself when the fixture moves to
// `Signature`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[expect(deprecated)]
mod tests {
    use super::*;
    use protocol::Slot;

    use crate::test_fixtures::genesis_store;

    /// Builds a 4-validator genesis store ready for `produce_block` /
    /// `produce_attestation_vote` calls.
    fn produce_setup() -> (Store, Bytes32) {
        genesis_store(4)
    }

    // -- produce_block ---------------------------------------------------

    #[test]
    fn produce_block_rejects_unauthorized_proposer() {
        let (mut store, _) = produce_setup();
        // For slot 1 with 4 validators, the proposer is index 1.
        let err = store
            .produce_block(Slot::new(1), ValidatorIndex::new(2))
            .unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::UnauthorizedProposer {
                validator: ValidatorIndex::new(2),
                slot: Slot::new(1),
            }
        );
    }

    #[test]
    fn produce_block_succeeds_at_slot_1_with_no_votes() {
        let (mut store, anchor_root) = produce_setup();
        let result = store
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .expect("produce_block");
        assert_eq!(result.parent_root, anchor_root);
        assert_eq!(result.block.parent_root, anchor_root);
        assert_eq!(result.block.slot, Slot::new(1));
        assert_eq!(result.block.proposer_index, ValidatorIndex::new(1));
        assert!(result.block.body.attestations.is_empty());
        assert_eq!(store.head(), result.root);
    }

    #[test]
    fn produce_block_state_root_matches_post_state_hash_tree_root() {
        let (mut store, _) = produce_setup();
        let result = store
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .expect("produce_block");
        let recomputed: Bytes32 = result.post_state.hash_tree_root().into();
        assert_eq!(result.block.state_root, recomputed);
        assert_eq!(result.post_state_root, recomputed);
    }

    #[test]
    fn produce_block_root_matches_hash_tree_root() {
        let (mut store, _) = produce_setup();
        let result = store
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .expect("produce_block");
        let recomputed: Bytes32 = result.block.hash_tree_root().into();
        assert_eq!(result.root, recomputed);
    }

    #[test]
    fn produce_block_tracks_result_in_store() {
        let (mut store, _) = produce_setup();
        let result = store
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .expect("produce_block");
        assert!(store.has_block(&result.root));
    }

    #[test]
    fn produce_block_post_state_slot_matches_block_slot() {
        let (mut store, _) = produce_setup();
        let result = store
            .produce_block(Slot::new(1), ValidatorIndex::new(1))
            .expect("produce_block");
        assert_eq!(result.post_state.slot, Slot::new(1));
    }

    // -- collect_includable_votes (MAX_ATTESTATIONS cap) ----------------

    #[test]
    fn collect_includable_votes_caps_at_max_attestations() {
        // Verify `.take(cap)` truncates when the pool is already at capacity.
        // We pass a synthetic `already_included` of length MAX_ATTESTATIONS;
        // the cap is then zero and the result must be empty regardless of
        // `latest_known_votes` content.
        let (store, _) = produce_setup();
        let dummy: Vec<SignedVote> = (0..MAX_ATTESTATIONS).map(|_| dummy_signed_vote()).collect();
        let votes = store.collect_includable_votes(&dummy_state(), &dummy);
        assert!(votes.is_empty());
    }

    fn dummy_signed_vote() -> SignedVote {
        use protocol::Vote;
        SignedVote {
            validator_id: ValidatorIndex::new(0),
            message: Vote {
                slot: Slot::ZERO,
                head: Checkpoint::default(),
                target: Checkpoint::default(),
                source: Checkpoint::default(),
            },
            signature: types::Bytes4000::new([0; 4000]),
        }
    }

    fn dummy_state() -> State {
        use crate::test_fixtures::genesis_anchor;
        genesis_anchor(4).0
    }

    // -- produce_attestation_vote ---------------------------------------

    #[test]
    fn produce_attestation_vote_at_slot_1() {
        let (mut store, anchor_root) = produce_setup();
        let result = store
            .produce_attestation_vote(Slot::new(1))
            .expect("produce_attestation_vote");
        // head is the genesis anchor; the store hasn't advanced past it.
        assert_eq!(result.head_root, anchor_root);
        assert_eq!(result.vote.slot, Slot::new(1));
        assert_eq!(result.vote.head.root, anchor_root);
        assert_eq!(result.vote.head.slot, Slot::ZERO);
        // No imported blocks → safe_target is also the anchor.
        assert_eq!(result.safe_target, anchor_root);
        // source mirrors the store's latest_justified, normalized to the
        // tracked genesis anchor.
        assert_eq!(result.source, store.latest_justified());
        assert_eq!(result.source.root, anchor_root);
    }

    #[test]
    fn produce_attestation_vote_target_matches_get_vote_target() {
        let (mut store, _) = produce_setup();
        let result = store
            .produce_attestation_vote(Slot::new(1))
            .expect("produce_attestation_vote");
        // With genesis head == safe_target, the walk is a no-op.
        assert_eq!(result.target.root, store.head());
        assert_eq!(result.target.slot, Slot::ZERO);
    }

    // -- advance_state_to_slot ------------------------------------------

    #[test]
    fn advance_state_to_slot_equal_is_noop() {
        let state = dummy_state();
        let advanced = advance_state_to_slot(state.clone(), Slot::ZERO).unwrap();
        assert_eq!(advanced.slot, Slot::ZERO);
    }

    #[test]
    fn advance_state_to_slot_forward_advances() {
        let state = dummy_state();
        let advanced = advance_state_to_slot(state, Slot::new(3)).unwrap();
        assert_eq!(advanced.slot, Slot::new(3));
    }

    #[test]
    fn advance_state_to_slot_backwards_errors() {
        let mut state = dummy_state();
        state.process_slots(Slot::new(5)).unwrap();
        let err = advance_state_to_slot(state, Slot::new(2)).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::StateSlotAheadOfTarget {
                state_slot: Slot::new(5),
                target_slot: Slot::new(2),
            }
        );
    }
}
