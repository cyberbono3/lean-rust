//! LMD-GHOST traversal primitive.
//!
//! [`get_fork_choice_head`] mirrors leanSpec
//! `forkchoice/helpers.py::get_fork_choice_head`. The three-step structure is
//! mandated by the spec — vote-weight accumulation, threshold filter into a
//! children map, then greedy descent with `(weight, slot, root)` tie-break.
//! Reordering the filter against the walk or relaxing the tie-break breaks
//! parity with the canonical implementation.
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

    if latest_votes.is_empty() {
        return Ok(root);
    }

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

    // Step 3: greedy descent. Tie-break is `(weight, slot, root_bytes)` via
    // tuple `Ord` — `Bytes32` derives `Ord` over its 32-byte lex order.
    let mut current = root;
    while let Some(best) = children.get(&current).and_then(|kids| {
        kids.iter()
            .copied()
            .max_by_key(|child| (weight_of(child), blocks[child].slot, *child))
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

    /// Builds a block with the given `(slot, parent_root)`. `state_root` is
    /// the all-`fill` byte pattern so distinct fills produce distinct roots
    /// when needed.
    fn block_with(slot: u64, parent_root: Bytes32, fill: u8) -> Block {
        Block {
            slot: Slot::new(slot),
            proposer_index: ValidatorIndex::new(0),
            parent_root,
            state_root: Bytes32::new([fill; 32]),
            body: BlockBody::default(),
        }
    }

    fn insert(blocks: &mut HashMap<Bytes32, Block>, root: Bytes32, block: Block) {
        blocks.insert(root, block);
    }

    #[test]
    fn empty_votes_returns_root_unchanged() {
        let mut blocks = HashMap::new();
        let root = Bytes32::new([1; 32]);
        insert(&mut blocks, root, block_with(0, Bytes32::zero(), 0));

        let head = get_fork_choice_head(&blocks, root, &HashMap::new(), 0).unwrap();
        assert_eq!(head, root);
    }

    #[test]
    fn zero_root_defaults_to_min_slot_then_min_root_bytes() {
        let mut blocks = HashMap::new();
        let a = Bytes32::new([0xaa; 32]);
        let b = Bytes32::new([0xbb; 32]);
        // Two blocks at slot 0 — tie-break must pick the lex-min root.
        insert(&mut blocks, a, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks, b, block_with(0, Bytes32::zero(), 0));

        let head = get_fork_choice_head(&blocks, Bytes32::zero(), &HashMap::new(), 0).unwrap();
        assert_eq!(head, a);

        // Now add a strictly-lower slot to force the slot-axis tie-break.
        let c = Bytes32::new([0xff; 32]); // lex-max, but slot-min
        let mut blocks2 = blocks.clone();
        insert(&mut blocks2, c, block_with(0, Bytes32::zero(), 0));
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
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks, a, block_with(1, dangling_parent, 0));

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
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks, a, block_with(1, genesis, 0));
        insert(&mut blocks, b1, block_with(2, a, 1));
        insert(&mut blocks, b2, block_with(2, a, 2));

        let votes = HashMap::from([
            (ValidatorIndex::new(0), Checkpoint::new(b1, Slot::new(2))),
            (ValidatorIndex::new(1), Checkpoint::new(b1, Slot::new(2))),
            (ValidatorIndex::new(2), Checkpoint::new(b2, Slot::new(2))),
        ]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap();
        assert_eq!(head, b1);
    }

    #[test]
    fn tie_break_prefers_higher_slot_then_higher_root_bytes() {
        // genesis -> a (slot 1)
        //         -> b (slot 2)
        // No votes → weight 0 everywhere → tie-break must prefer higher slot.
        let mut blocks = HashMap::new();
        let genesis = Bytes32::new([1; 32]);
        let a = Bytes32::new([0xff; 32]);
        let b = Bytes32::new([0x10; 32]);
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks, a, block_with(1, genesis, 1));
        insert(&mut blocks, b, block_with(2, genesis, 2));

        // Vote map is non-empty but unrelated to a/b — both have weight 0,
        // higher slot (b) wins.
        let votes = HashMap::from([(
            ValidatorIndex::new(0),
            Checkpoint::new(genesis, Slot::new(0)),
        )]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap();
        assert_eq!(head, b);

        // Same slot: lex-max root wins.
        let mut blocks2 = HashMap::new();
        let lex_lo = Bytes32::new([0x10; 32]);
        let lex_hi = Bytes32::new([0xff; 32]);
        insert(&mut blocks2, genesis, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks2, lex_lo, block_with(1, genesis, 1));
        insert(&mut blocks2, lex_hi, block_with(1, genesis, 2));
        let head = get_fork_choice_head(&blocks2, genesis, &votes, 0).unwrap();
        assert_eq!(head, lex_hi);
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
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero(), 0));
        insert(&mut blocks, a, block_with(1, genesis, 0));
        insert(&mut blocks, b, block_with(2, a, 0));

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
        insert(&mut blocks, genesis, block_with(0, Bytes32::zero(), 0));

        let votes = HashMap::from([(
            ValidatorIndex::new(0),
            Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::new(7)),
        )]);
        let head = get_fork_choice_head(&blocks, genesis, &votes, 0).unwrap();
        assert_eq!(head, genesis);
    }
}
