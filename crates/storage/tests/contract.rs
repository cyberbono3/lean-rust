//! Generic contract suite for the `storage::Store` trait.
//!
//! [`run_store_contract`] takes a `factory: impl Fn() -> S` and runs every
//! scenario against a freshly-built store, so cross-scenario state leakage
//! is impossible. Future adapters inherit the entire suite by calling
//! `run_store_contract(MyAdapter::new)`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_const_for_fn,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use static_assertions::{assert_impl_all, assert_obj_safe};
use storage::{MemoryStore, Store};

use fixtures::storage::{sample_head, sample_root, sample_signed_block, sample_state};

// =============================================================================
// Generic contract — call from any adapter's integration test
// =============================================================================

/// Runs the full `Store`-contract suite. Each scenario receives a fresh
/// store from `factory`, so cross-scenario state leakage is impossible.
pub fn run_store_contract<S: Store>(factory: impl Fn() -> S) {
    block_round_trip(&factory());
    state_round_trip(&factory());
    head_round_trip(&factory());
    has_block_false_for_unknown(&factory());
    has_block_true_after_save(&factory());
    load_block_none_for_unknown(&factory());
    load_state_none_for_unknown(&factory());
    load_head_none_before_first_save(&factory());
    save_block_overwrites_existing_root(&factory());
    save_state_overwrites_existing_root(&factory());
    save_head_overwrites_previous_head(&factory());
}

fn block_round_trip(store: &impl Store) {
    let root = sample_root(1);
    let block = sample_signed_block(1);
    store.save_block(root, block.clone()).unwrap();
    assert_eq!(store.load_block(&root).unwrap(), Some(block));
}

fn state_round_trip(store: &impl Store) {
    let root = sample_root(1);
    let state = sample_state(1);
    store.save_state(root, state.clone()).unwrap();
    assert_eq!(store.load_state(&root).unwrap(), Some(state));
}

fn head_round_trip(store: &impl Store) {
    let info = sample_head(1);
    store.save_head(info).unwrap();
    assert_eq!(store.load_head().unwrap(), Some(info));
}

fn has_block_false_for_unknown(store: &impl Store) {
    assert!(!store.has_block(&sample_root(7)).unwrap());
}

fn has_block_true_after_save(store: &impl Store) {
    let root = sample_root(1);
    store.save_block(root, sample_signed_block(1)).unwrap();
    assert!(store.has_block(&root).unwrap());
}

fn load_block_none_for_unknown(store: &impl Store) {
    assert_eq!(store.load_block(&sample_root(7)).unwrap(), None);
}

fn load_state_none_for_unknown(store: &impl Store) {
    assert_eq!(store.load_state(&sample_root(7)).unwrap(), None);
}

fn load_head_none_before_first_save(store: &impl Store) {
    // Now a real test: each scenario gets a fresh store, so `head` is
    // genuinely unset on entry.
    assert_eq!(store.load_head().unwrap(), None);
}

fn save_block_overwrites_existing_root(store: &impl Store) {
    let root = sample_root(1);
    store.save_block(root, sample_signed_block(1)).unwrap();
    store.save_block(root, sample_signed_block(2)).unwrap();
    assert_eq!(
        store.load_block(&root).unwrap(),
        Some(sample_signed_block(2))
    );
}

fn save_state_overwrites_existing_root(store: &impl Store) {
    let root = sample_root(1);
    store.save_state(root, sample_state(1)).unwrap();
    store.save_state(root, sample_state(2)).unwrap();
    assert_eq!(store.load_state(&root).unwrap(), Some(sample_state(2)));
}

fn save_head_overwrites_previous_head(store: &impl Store) {
    store.save_head(sample_head(1)).unwrap();
    store.save_head(sample_head(2)).unwrap();
    assert_eq!(store.load_head().unwrap(), Some(sample_head(2)));
}

// =============================================================================
// MemoryStore-specific tests (drive the generic suite + concurrency + asserts)
// =============================================================================

#[test]
fn memory_store_passes_contract() {
    run_store_contract(MemoryStore::new);
}

#[test]
fn store_is_object_safe_and_send_sync() {
    assert_obj_safe!(Store);
    assert_impl_all!(MemoryStore: Store, Send, Sync);
}

#[test]
fn arc_dyn_store_dispatches_through_vtable() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let root = sample_root(42);
    store.save_block(root, sample_signed_block(42)).unwrap();
    assert!(store.has_block(&root).unwrap());
    assert_eq!(
        store.load_block(&root).unwrap(),
        Some(sample_signed_block(42))
    );
}

#[test]
fn arc_memory_store_concurrent_save_and_load() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    std::thread::scope(|scope| {
        for i in 0..8_u8 {
            let store = Arc::clone(&store);
            scope.spawn(move || {
                let root = sample_root(i);
                store.save_block(root, sample_signed_block(i)).unwrap();
                assert_eq!(
                    store.load_block(&root).unwrap(),
                    Some(sample_signed_block(i))
                );
            });
        }
    });

    for i in 0..8_u8 {
        assert!(store.has_block(&sample_root(i)).unwrap());
    }
}
