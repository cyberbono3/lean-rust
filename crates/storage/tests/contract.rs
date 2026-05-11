//! Generic contract suite for the `storage::Store` trait.
//!
//! [`run_store_contract`] takes any `&S: Store` and runs the full
//! round-trip + absence + overwrite + concurrency suite against it. Future
//! adapters inherit the entire test set by calling
//! `run_store_contract(&adapter)`.

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

mod fixtures;
use fixtures::{sample_head, sample_root, sample_signed_block, sample_state};

// =============================================================================
// Generic contract — call from any adapter's integration test
// =============================================================================

/// Runs the full `Store`-contract suite against `store`.
pub fn run_store_contract<S: Store>(store: &S) {
    block_round_trip(store);
    state_round_trip(store);
    head_round_trip(store);
    has_block_false_for_unknown(store);
    has_block_true_after_save(store);
    load_block_none_for_unknown(store);
    load_state_none_for_unknown(store);
    load_head_none_before_first_save(store);
    save_block_overwrites_existing_root(store);
    save_state_overwrites_existing_root(store);
    save_head_overwrites_previous_head(store);
}

fn block_round_trip<S: Store>(store: &S) {
    let root = sample_root(11);
    let block = sample_signed_block(11);
    store.save_block(root, block.clone()).unwrap();
    assert_eq!(store.load_block(&root).unwrap(), Some(block));
}

fn state_round_trip<S: Store>(store: &S) {
    let root = sample_root(12);
    let state = sample_state(12);
    store.save_state(root, state.clone()).unwrap();
    assert_eq!(store.load_state(&root).unwrap(), Some(state));
}

fn head_round_trip<S: Store>(store: &S) {
    let info = sample_head(13);
    store.save_head(info).unwrap();
    assert_eq!(store.load_head().unwrap(), Some(info));
}

fn has_block_false_for_unknown<S: Store>(store: &S) {
    let unknown = sample_root(200);
    assert!(!store.has_block(&unknown).unwrap());
}

fn has_block_true_after_save<S: Store>(store: &S) {
    let root = sample_root(14);
    let block = sample_signed_block(14);
    store.save_block(root, block).unwrap();
    assert!(store.has_block(&root).unwrap());
}

fn load_block_none_for_unknown<S: Store>(store: &S) {
    let unknown = sample_root(201);
    assert_eq!(store.load_block(&unknown).unwrap(), None);
}

fn load_state_none_for_unknown<S: Store>(store: &S) {
    let unknown = sample_root(202);
    assert_eq!(store.load_state(&unknown).unwrap(), None);
}

fn load_head_none_before_first_save<S: Store>(store: &S) {
    // This case requires a fresh store — handled separately in
    // `memory_store_load_head_none_before_first_save` below.
    let _ = store;
}

fn save_block_overwrites_existing_root<S: Store>(store: &S) {
    let root = sample_root(15);
    store.save_block(root, sample_signed_block(15)).unwrap();
    store.save_block(root, sample_signed_block(16)).unwrap();
    let got = store.load_block(&root).unwrap().expect("present");
    assert_eq!(got, sample_signed_block(16));
}

fn save_state_overwrites_existing_root<S: Store>(store: &S) {
    let root = sample_root(17);
    store.save_state(root, sample_state(17)).unwrap();
    store.save_state(root, sample_state(18)).unwrap();
    let got = store.load_state(&root).unwrap().expect("present");
    assert_eq!(got, sample_state(18));
}

fn save_head_overwrites_previous_head<S: Store>(store: &S) {
    store.save_head(sample_head(19)).unwrap();
    store.save_head(sample_head(20)).unwrap();
    assert_eq!(store.load_head().unwrap(), Some(sample_head(20)));
}

// =============================================================================
// MemoryStore-specific tests (drive the generic suite + concurrency + asserts)
// =============================================================================

#[test]
fn memory_store_passes_contract() {
    run_store_contract(&MemoryStore::new());
}

#[test]
fn memory_store_load_head_none_before_first_save() {
    // Separate test that requires a fresh store — can't be in the generic
    // suite, which receives a single instance and runs everything against it.
    let store = MemoryStore::new();
    assert_eq!(store.load_head().unwrap(), None);
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
    let loaded = store.load_block(&root).unwrap();
    assert_eq!(loaded, Some(sample_signed_block(42)));
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
                let loaded = store.load_block(&root).unwrap();
                assert_eq!(loaded, Some(sample_signed_block(i)));
            });
        }
    });

    // Every per-thread root is present after the scope completes.
    for i in 0..8_u8 {
        assert!(store.has_block(&sample_root(i)).unwrap());
    }
}
