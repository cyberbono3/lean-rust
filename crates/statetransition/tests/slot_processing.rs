//! End-to-end coverage that the [`protocol::State`] slot-processing methods
//! work on a state produced by [`statetransition::genesis_state`]. Unit
//! coverage of `process_slot` / `process_slots` lives in
//! `protocol::transition`; this file pins the genesis → advance path
//! across the crate boundary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use protocol::{Slot, StateTransitionError};
use ssz::HashTreeRoot;
use statetransition::genesis_state;
use types::Bytes32;

const NUM_VALIDATORS: u64 = 4;
const GENESIS_TIME: u64 = 1_700_000_000;

fn fresh() -> protocol::State {
    genesis_state(NUM_VALIDATORS, GENESIS_TIME)
}

#[test]
fn genesis_then_process_slots_reaches_target() {
    let mut state = fresh();
    state.process_slots(Slot::new(5)).unwrap();
    assert_eq!(state.slot, Slot::new(5));
}

#[test]
fn genesis_then_process_slots_caches_pre_advance_state_root() {
    let snapshot = fresh();
    let pre_root: Bytes32 = snapshot.hash_tree_root().into();
    let mut state = fresh();
    state.process_slots(Slot::new(3)).unwrap();
    assert_eq!(state.latest_block_header.state_root, pre_root);
}

#[test]
fn genesis_then_process_slots_rejects_zero_target() {
    let mut state = fresh();
    let err = state.process_slots(Slot::ZERO).unwrap_err();
    assert_eq!(
        err,
        StateTransitionError::TargetSlotNotInFuture {
            current: Slot::ZERO,
            target: Slot::ZERO,
        }
    );
}
