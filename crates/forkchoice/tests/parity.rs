//! LMD-GHOST head-traversal parity vectors.
//!
//! Hand-derived cases — see `tests/data/head_traversal/PROVENANCE.md` for
//! the rationale. Each case is a `(name, blocks, votes, root, min_score,
//! expected)` tuple replayed against `helpers::get_fork_choice_head`.
//!
//! The vectors collectively exercise:
//! - Linear chain without votes (deepest block wins via slot tie-break).
//! - Two-fork supermajority routing weight to the heavier subtree.
//! - Tie-break ordering: `(weight, slot, root_bytes)`.
//! - `min_score` filtering an under-threshold subtree.
//! - Origin defaulting to `min_block_root` when `root == Bytes32::zero()`.
//! - Error surfaces for unknown roots and empty block sets.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use forkchoice::helpers::get_fork_choice_head;
use forkchoice::ForkchoiceError;
use protocol::{Block, BlockBody, Checkpoint, Slot, ValidatorIndex};
use types::Bytes32;

#[allow(clippy::expect_used)]
fn block(slot: u64, parent_root: Bytes32, fill: u8) -> Block {
    Block {
        slot: Slot::new(slot),
        proposer_index: ValidatorIndex::new(0),
        parent_root,
        state_root: Bytes32::new([fill; 32]),
        body: BlockBody::default(),
    }
}

fn root(byte: u8) -> Bytes32 {
    Bytes32::new([byte; 32])
}

#[test]
fn parity_linear_chain_no_votes_picks_deepest() {
    // genesis (slot 0) → a (slot 1) → b (slot 2). With no votes, descent
    // follows the slot tie-break in `max_by_key` and lands at the deepest.
    let g = root(0x01);
    let a = root(0x02);
    let b = root(0x03);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero(), 0)),
        (a, block(1, g, 1)),
        (b, block(2, a, 2)),
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
        (g, block(0, Bytes32::zero(), 0)),
        (a, block(1, g, 0)),
        (b1, block(2, a, 0)),
        (b2, block(2, a, 0)),
    ]);
    let votes = HashMap::from([
        (ValidatorIndex::new(0), Checkpoint::new(b1, Slot::new(2))),
        (ValidatorIndex::new(1), Checkpoint::new(b1, Slot::new(2))),
        (ValidatorIndex::new(2), Checkpoint::new(b2, Slot::new(2))),
    ]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), b1);
}

#[test]
fn parity_tie_break_prefers_higher_slot() {
    // Two children of genesis at different slots, both zero weight → the
    // higher-slot child wins.
    let g = root(0x01);
    let lo = root(0xff); // lex-max, but lower slot
    let hi = root(0x10); // lex-min, but higher slot
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero(), 0)),
        (lo, block(1, g, 0)),
        (hi, block(2, g, 0)),
    ]);
    let votes = HashMap::from([(ValidatorIndex::new(0), Checkpoint::new(g, Slot::ZERO))]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), hi);
}

#[test]
fn parity_tie_break_prefers_higher_root_when_slot_equal() {
    let g = root(0x01);
    let lo = root(0x10);
    let hi = root(0xff);
    let blocks = HashMap::from([
        (g, block(0, Bytes32::zero(), 0)),
        (lo, block(1, g, 0)),
        (hi, block(1, g, 0)),
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
        (g, block(0, Bytes32::zero(), 0)),
        (a, block(1, g, 0)),
        (b, block(2, a, 0)),
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
        (a, block(2, Bytes32::zero(), 0)),
        (b, block(0, Bytes32::zero(), 0)),
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
    let blocks = HashMap::from([(g, block(0, Bytes32::zero(), 0))]);
    let votes = HashMap::from([(
        ValidatorIndex::new(0),
        Checkpoint::new(root(0xaa), Slot::new(7)),
    )]);
    assert_eq!(get_fork_choice_head(&blocks, g, &votes, 0).unwrap(), g);
}
