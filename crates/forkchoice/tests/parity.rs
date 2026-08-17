//! LMD-GHOST head-traversal parity vectors.
//!
//! Hand-derived cases — see `tests/data/head_traversal/PROVENANCE.md` for
//! the rationale. Each case is a `(name, blocks, votes, root, min_score,
//! expected)` tuple replayed against `helpers::get_fork_choice_head`.
//!
//! These vectors deliberately overlap the unit tests in `helpers.rs`. They
//! are not duplicates: the vectors here stand in for an upstream trajectory
//! fixture and are replaced by a replay of it once the block-import path
//! lands, while the unit tests stay as isolated regression coverage. Neither
//! suite is redundant with the other.
//!
//! The vectors collectively exercise:
//! - Linear chain without votes (single child per level — no tie to break).
//! - Two-fork supermajority routing weight to the heavier subtree.
//! - Tie-break ordering: `(weight, root_bytes)`, with slot absent.
//! - `min_score` filtering an under-threshold subtree.
//! - Origin defaulting to `min_block_root` when `root == Bytes32::zero()`.
//! - Error surfaces for unknown roots and empty block sets.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use forkchoice::helpers::get_fork_choice_head;
use forkchoice::ForkchoiceError;
use protocol::{Block, BlockBody, Checkpoint, Slot, ValidatorIndex};
use types::Bytes32;

fn block(slot: u64, parent_root: Bytes32) -> Block {
    Block {
        slot: Slot::new(slot),
        proposer_index: ValidatorIndex::new(0),
        parent_root,
        state_root: Bytes32::zero(),
        body: BlockBody::default(),
    }
}

const fn root(byte: u8) -> Bytes32 {
    Bytes32::new([byte; 32])
}

#[test]
fn parity_linear_chain_no_votes_picks_deepest() {
    // genesis (slot 0) → a (slot 1) → b (slot 2). Every level has a single
    // child, so the descent never reaches a fork and the tie-break is never
    // exercised — it lands at the deepest block either way.
    let g = root(0x01);
    let a = root(0x02);
    let b = root(0x03);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero())),
        (a, block(1, g)),
        (b, block(2, a)),
    ]);
    let votes = HashMap::new();
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), b);
    // Sanity: with one vote pointing at `b`, the descent reaches `b`.
    let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(b, Slot::new(2)))]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), b);
}

#[test]
fn parity_two_fork_supermajority_routes_weight() {
    // genesis → a → {b1, b2}. Two voters at b1, one at b2 → head is b1.
    let g = root(0x01);
    let a = root(0x02);
    let b1 = root(0x03);
    let b2 = root(0x04);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero())),
        (a, block(1, g)),
        (b1, block(2, a)),
        (b2, block(2, a)),
    ]);
    let votes = HashMap::from([
        (ValidatorIndex::new(0), Checkpoint::new(b1, Slot::new(2))),
        (ValidatorIndex::new(1), Checkpoint::new(b1, Slot::new(2))),
        (ValidatorIndex::new(2), Checkpoint::new(b2, Slot::new(2))),
    ]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), b1);
}

#[test]
fn head_tie_breaks_on_root() {
    // leanSpec `forkchoice/store.py:746 @ 0c9528ac`: the descent picks
    // `max(children, key=lambda x: (weights[x], x))`. The key is
    // (weight, root); slot is not in it.
    //
    // This fixture puts the lex-larger root at the LOWER slot, so a key that
    // ranked slot ahead of root would answer `deeper` instead. That
    // disagreement is the whole point of the vector — a fixture whose
    // lex-max root also sits at the higher slot passes under either key and
    // pins nothing.
    let g = root(0x01);
    let shallower = root(0xff); // slot 1, lex-max → the spec's winner
    let deeper = root(0x10); // slot 2, lex-min
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero())),
        (shallower, block(1, g)),
        (deeper, block(2, g)),
    ]);
    // One vote on the origin leaves both children at weight 0.
    let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(g, Slot::ZERO))]);

    let head = get_fork_choice_head(&blocks, g, &votes, 0).unwrap();
    assert_eq!(head, shallower);
    // Non-vacuity: the winner really is the shallower of the two, so this
    // cannot be misread as a same-slot case that would pass either way.
    assert!(blocks[&head].slot < blocks[&deeper].slot);
}

#[test]
fn parity_tie_break_prefers_higher_root_when_slot_equal() {
    let g = root(0x01);
    let lo = root(0x10);
    let hi = root(0xff);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero())),
        (lo, block(1, g)),
        (hi, block(1, g)),
    ]);
    let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(g, Slot::ZERO))]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), hi);
}

#[test]
fn parity_min_score_filters_under_threshold_subtree() {
    // genesis → a → b. Single vote at b → weight(a) = weight(b) = 1.
    // With min_score = 2, both are filtered out; descent stops at genesis.
    let g = root(0x01);
    let a = root(0x02);
    let b = root(0x03);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero())),
        (a, block(1, g)),
        (b, block(2, a)),
    ]);
    let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(b, Slot::new(2)))]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 2).unwrap(), g);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 1).unwrap(), b);
}

#[test]
fn parity_zero_root_defaults_to_min_block() {
    let a = root(0x05); // higher slot, lex-min vs b
    let b = root(0xff); // lower slot, lex-max
    let blocks = HashMap::from([
        (a, block(2, Bytes32::zero())),
        (b, block(0, Bytes32::zero())),
    ]);
    // Default origin walks `min_block_root` (slot asc, root asc):
    // slot 0 wins → origin = b.
    let votes = HashMap::new();
    assert_eq!(
        get_fork_choice_head(&blocks, Bytes32::zero(), &votes, 0).unwrap(),
        b
    );
}

#[test]
fn parity_empty_block_set_with_zero_root_errors() {
    let blocks: HashMap<Bytes32, Block> = HashMap::new();
    let votes = HashMap::new();
    let err = get_fork_choice_head(&blocks, Bytes32::zero(), &votes, 0).unwrap_err();
    assert!(matches!(err, ForkchoiceError::NoBlocksAvailable));
}

#[test]
fn parity_unknown_root_errors() {
    let blocks: HashMap<Bytes32, Block> = HashMap::new();
    let votes = HashMap::new();
    let bogus = root(0x77);
    let err = get_fork_choice_head(&blocks, bogus, &votes, 0).unwrap_err();
    assert_eq!(err, ForkchoiceError::UnknownRootBlock { root: bogus });
}

#[test]
fn parity_vote_to_unknown_block_is_silently_skipped() {
    let g = root(0x01);
    let blocks = HashMap::from([(g, block(0, Bytes32::zero()))]);
    let votes = HashMap::from([(
        ValidatorIndex::new(0),
        Checkpoint::new(root(0xaa), Slot::new(7)),
    )]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), g);
}
