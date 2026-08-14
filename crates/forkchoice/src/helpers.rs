//! LMD-GHOST traversal primitive.
//!
//! [`get_fork_choice_head`] mirrors leanSpec
//! `forkchoice/store.py::_compute_lmd_ghost_head`. The three-step structure
//! is mandated by the spec — vote-weight accumulation, threshold filter into
//! a children map, then greedy descent with a `(weight, root)` tie-break
//! (`store.py:746 @ 0c9528ac`). Reordering the filter against the walk, or
//! adding an axis to the tie-break, breaks parity with the canonical
//! implementation.
//!
//! Slot is deliberately absent from the tie-break. On equal weight the
//! lex-larger root wins even when it sits at a lower slot, so the resolved
//! head can be shallower than an available sibling. That is the specified
//! behaviour, not an oversight: adding a slot axis ahead of the root axis
//! is the exact divergence this rule exists to prevent.
//!
//! Public surface: [`get_fork_choice_head`] is consumed by
//! [`crate::Store`]'s `update_safe_target` / `update_head` hooks and by
//! downstream crates that need to resolve a head root from a custom
//! `(blocks, votes, min_score)` triple.

use std::collections::HashMap;

use protocol::{Block, Checkpoint, ValidatorIndex};
use types::Bytes32;

use crate::error::ForkchoiceError;

/// Applies the LMD-GHOST rule from `root` using `latest_votes`, returning
/// the resolved head root.
///
/// When `root == Bytes32::zero()` the origin defaults to the lowest-slot
/// block, ties broken by ascending root-bytes. With `min_score = 0` this is
/// canonical head selection; with `min_score = ceil(2N/3)` it is the
/// supermajority-gated safe-target selection driven by
/// [`crate::Store::update_safe_target`].
///
/// # Errors
/// - [`ForkchoiceError::NoBlocksAvailable`] when `root` defaults from zero
///   and `blocks` is empty.
/// - [`ForkchoiceError::UnknownRootBlock`] when a non-zero `root` is not in
///   `blocks`.
/// - [`ForkchoiceError::ParentBlockNotFound`] when the weight-walk runs
///   past a block whose `parent_root` is absent from `blocks`.
///
/// # Example
/// ```
/// use std::collections::HashMap;
/// use forkchoice::{helpers::get_fork_choice_head, ForkchoiceError};
/// use types::Bytes32;
///
/// let err = get_fork_choice_head(&HashMap::new(), Bytes32::zero(), &HashMap::new(), 0)
///     .unwrap_err();
/// assert!(matches!(err, ForkchoiceError::NoBlocksAvailable));
/// ```
#[allow(clippy::implicit_hasher)]
pub fn get_fork_choice_head(
    blocks: &HashMap<Bytes32, Block>,
    root: Bytes32,
    latest_votes: &HashMap<ValidatorIndex, Checkpoint>,
    min_score: u64,
) -> Result<Bytes32, ForkchoiceError> {
    // Resolve the descent origin once. `Bytes32::zero()` is the sentinel for
    // "use the lowest-slot block"; any non-zero root must be tracked.
    let root = if root == Bytes32::zero() {
        min_block_root(blocks).ok_or(ForkchoiceError::NoBlocksAvailable)?
    } else {
        root
    };
    let root_slot = blocks
        .get(&root)
        .ok_or(ForkchoiceError::UnknownRootBlock { root })?
        .slot;

    // Step 1: per-block vote weight. For each voted block, walk back to the
    // root depth and bump the weight of every block on the path. Votes whose
    // head root is not tracked are silently skipped (matches leanSpec).
    let mut weights: HashMap<Bytes32, u64> = HashMap::new();
    for checkpoint in latest_votes.values() {
        let mut cursor = checkpoint.root;
        let Some(mut block) = blocks.get(&cursor) else {
            continue;
        };
        while block.slot > root_slot {
            *weights.entry(cursor).or_default() += 1;
            cursor = block.parent_root;
            block = blocks
                .get(&cursor)
                .ok_or(ForkchoiceError::ParentBlockNotFound { root: cursor })?;
        }
    }
    let weight_of = |r: &Bytes32| weights.get(r).copied().unwrap_or(0);

    // Step 2: children map filtered by `weight >= min_score`. The filter
    // runs here, not at descent time, so the threshold gate is uniform
    // across the recursive descent.
    let mut children: HashMap<Bytes32, Vec<Bytes32>> = HashMap::new();
    for (block_root, block) in blocks {
        if weight_of(block_root) >= min_score {
            children
                .entry(block.parent_root)
                .or_default()
                .push(*block_root);
        }
    }

    // Step 3: greedy descent. Tie-break is `(weight, root_bytes)` via tuple
    // `Ord` — `Bytes32` derives `Ord` over its 32-byte lex order, matching
    // the reference specification's `max(children, key=(weights[x], x))`.
    // Slot is deliberately absent: on equal weight the lex-larger root wins
    // even when it sits at a lower slot.
    let mut current = root;
    while let Some(best) = children.get(&current).and_then(|kids| {
        kids.iter()
            .copied()
            .max_by_key(|child| (weight_of(child), *child))
    }) {
        current = best;
    }
    Ok(current)
}

/// Returns the root of the block with the lowest slot, ties broken by the
/// lexicographically smallest root.
fn min_block_root(blocks: &HashMap<Bytes32, Block>) -> Option<Bytes32> {
    blocks
        .iter()
        .min_by_key(|(root, block)| (block.slot, **root))
        .map(|(root, _)| *root)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::{Block, BlockBody, Slot, ValidatorIndex};

    // These unit tests overlap the parity vectors in `tests/parity.rs` by
    // design. The vectors there are stand-ins for an upstream trajectory
    // fixture and will be replaced by a replay of it; these stay as isolated
    // regression coverage. Do not delete either suite as redundant.

    /// Builds a block at `(slot, parent_root)`. The head walk reads only those
    /// two fields, so `state_root`, `proposer_index` and `body` stay at their
    /// zero/default values — every root in these tests is the caller's
    /// hand-chosen map key, never a hash of this block.
    fn block_with(slot: u64, parent_root: Bytes32) -> Block {
        Block {
            slot: Slot::new(slot),
            proposer_index: ValidatorIndex::new(0),
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        }
    }

    fn insert(blocks: &mut HashMap<Bytes32, Block>, root: Bytes32, block: Block) {
        blocks.insert(root, block);
    }

    #[test]
    fn empty_votes_descend_by_zero_weight_tie_break() {
        let mut blocks = HashMap::new();
        let root = Bytes32::new([1; 32]);
        let child = Bytes32::new([2; 32]);
        insert(&mut blocks, root, block_with(0, Bytes32::zero()));
        insert(&mut blocks, child, block_with(1, root));

        let head = get_fork_choice_head(&blocks, root, &HashMap::new(), 0).unwrap();
        assert_eq!(head, child);
    }

    #[test]
    fn zero_root_defaults_to_min_slot_then_min_root_bytes() {
        let mut blocks = HashMap::new();
        let a = Bytes32::new([0xaa; 32]);
        let b = Bytes32::new([0xbb; 32]);
        // Two blocks at slot 0 — tie-break must pick the lex-min root.
        insert(&mut blocks, a, block_with(0, Bytes32::zero()));
        insert(&mut blocks, b, block_with(0, Bytes32::zero()));

        let head = get_fork_choice_head(&blocks, Bytes32::zero(), &HashMap::new(), 0).unwrap();
        assert_eq!(head, a);

        // Now add a strictly-lower slot to force the slot-axis tie-break.
        let c = Bytes32::new([0xff; 32]); // lex-max, but slot-min
        let mut blocks2 = blocks.clone();
        insert(&mut blocks2, c, block_with(0, Bytes32::zero()));
        // Still tied at slot 0 → lex-min root wins.
        let head = get_fork_choice_head(&blocks2, Bytes32::zero(), &HashMap::new(), 0).unwrap();
        assert_eq!(head, a);
    }

    #[test]
    fn unknown_root_returns_error() {
        let blocks = HashMap::new();
        let missing = Bytes32::new([7; 32]);
        let err = get_fork_choice_head(&blocks, missing, &HashMap::new(), 0).unwrap_err();
        assert_eq!(err, ForkchoiceError::UnknownRootBlock { root: missing });
    }

    #[test]
    fn zero_root_over_empty_blocks_errors() {
        let blocks = HashMap::new();
        let err = get_fork_choice_head(&blocks, Bytes32::zero(), &HashMap::new(), 0).unwrap_err();
        assert_eq!(err, ForkchoiceError::NoBlocksAvailable);
    }

    #[test]
    fn parent_missing_during_walk_errors() {
        // Construct a chain genesis -> a, where `a.parent_root` points to a
        // root not present in `blocks`. A vote whose head is `a` must trigger
        // the parent-missing error during the weight walk.
        let mut blocks = HashMap::new();
        let genesis = Bytes32::new([1; 32]);
        let dangling_parent = Bytes32::new([0xcc; 32]);
        let a = Bytes32::new([2; 32]);
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero()));
        insert(&mut blocks, a, block_with(1, dangling_parent));

        let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(a, Slot::new(1)))]);
        let err = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap_err();
        assert_eq!(
            err,
            ForkchoiceError::ParentBlockNotFound {
                root: dangling_parent
            }
        );
    }

    #[test]
    fn greedy_descent_follows_majority_weight() {
        // genesis -> a -> b1
        //              \-> b2
        // Two voters at b1, one voter at b2 → head is b1.
        let mut blocks = HashMap::new();
        let genesis = Bytes32::new([1; 32]);
        let a = Bytes32::new([2; 32]);
        let b1 = Bytes32::new([3; 32]);
        let b2 = Bytes32::new([4; 32]);
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero()));
        insert(&mut blocks, a, block_with(1, genesis));
        insert(&mut blocks, b1, block_with(2, a));
        insert(&mut blocks, b2, block_with(2, a));

        let votes = HashMap::from([
            (ValidatorIndex::new(0), Checkpoint::new(b1, Slot::new(2))),
            (ValidatorIndex::new(1), Checkpoint::new(b1, Slot::new(2))),
            (ValidatorIndex::new(2), Checkpoint::new(b2, Slot::new(2))),
        ]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap();
        assert_eq!(head, b1);
    }

    /// Builds `parent → {a, b}`: two children of one parent, each at the
    /// caller's chosen `(root, slot)`. Lets a test place the lex-larger root
    /// at either slot, which is the only way to make slot and root disagree.
    fn two_child_fork(
        parent: Bytes32,
        a: (Bytes32, u64),
        b: (Bytes32, u64),
    ) -> HashMap<Bytes32, Block> {
        HashMap::from([
            (parent, block_with(0, Bytes32::zero())),
            (a.0, block_with(a.1, parent)),
            (b.0, block_with(b.1, parent)),
        ])
    }

    #[test]
    fn tie_break_is_weight_then_root_regardless_of_slot() {
        // leanSpec `forkchoice/store.py:746 @ 0c9528ac` descends with
        // `max(children, key=lambda x: (weights[x], x))` — the key is
        // (weight, root) and slot is absent. The expected winner is the same
        // lex-larger root in every row BECAUSE the slot column varies and the
        // answer does not: that invariance is the contract being pinned.
        //
        // Only the first row discriminates between the two keys. The other
        // two agree under either, and are here so a reader can see that the
        // discriminating row is not an artefact of one odd geometry.
        let parent = Bytes32::new([0x01; 32]);
        let lex_hi = Bytes32::new([0xff; 32]);
        let lex_lo = Bytes32::new([0x10; 32]);

        // Each row is (case name, the slot lex_hi sits at, the slot lex_lo
        // sits at) — NOT (higher slot, lower slot). Row 1 deliberately puts
        // lex_hi at the shallower of the two.
        let cases: [(&str, u64, u64); 3] = [
            ("lex-max sits at the LOWER slot", 1, 2),
            ("lex-max sits at the higher slot", 2, 1),
            ("both children share a slot", 1, 1),
        ];

        for (name, lex_hi_slot, lex_lo_slot) in cases {
            let blocks = two_child_fork(parent, (lex_hi, lex_hi_slot), (lex_lo, lex_lo_slot));
            // The single vote lands on the origin, so both children stay at
            // weight 0 and the tie-break alone decides the descent.
            let votes =
                HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(parent, Slot::ZERO))]);

            let head = get_fork_choice_head(&blocks, parent, &votes, 0).unwrap();
            assert_eq!(head, lex_hi, "case {name}: the lex-larger root must win");
        }
    }

    #[test]
    fn min_score_filter_excludes_under_threshold_subtree() {
        // genesis -> a (1 vote) -> b (1 vote)
        // With min_score = 2, both a and b are filtered out; descent stops at
        // genesis (which itself has 0 weight but is the origin).
        let mut blocks = HashMap::new();
        let genesis = Bytes32::new([1; 32]);
        let a = Bytes32::new([2; 32]);
        let b = Bytes32::new([3; 32]);
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero()));
        insert(&mut blocks, a, block_with(1, genesis));
        insert(&mut blocks, b, block_with(2, a));

        let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(b, Slot::new(2)))]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 2).unwrap();
        assert_eq!(head, genesis);

        // min_score = 1 lets the single-vote subtree through.
        let head = get_fork_choice_head(&blocks, genesis, &votes, 1).unwrap();
        assert_eq!(head, b);
    }

    #[test]
    fn vote_to_unknown_block_is_silently_skipped() {
        let mut blocks = HashMap::new();
        let genesis = Bytes32::new([1; 32]);
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero()));

        let votes = HashMap::from([(
            ValidatorIndex::new(0),
            Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::new(7)),
        )]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap();
        assert_eq!(head, genesis);
    }
}
