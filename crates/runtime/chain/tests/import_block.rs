//! Integration tests for `Service::import_block`.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use lean_chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use lean_chain::engine::{BlockImportResult, Engine};
use lean_chain::Service;
use protocol::{Block, BlockBody, SignedBlock, Slot, ValidatorIndex};
use ssz::HashTreeRoot;
use storage::{HeadInfo, MemoryStore, StorageError, Store};
use types::{Bytes32, Bytes4000};

/// `Store` decorator that counts each `save_*` invocation.
///
/// Wraps a [`MemoryStore`] and exposes per-method counters so tests can
/// assert that early-return import branches (`DuplicateBlock`,
/// `MissingParent`) skip persistence.
struct CountingStore {
    inner: MemoryStore,
    save_block_calls: AtomicUsize,
    save_state_calls: AtomicUsize,
    save_head_calls: AtomicUsize,
}

impl CountingStore {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            save_block_calls: AtomicUsize::new(0),
            save_state_calls: AtomicUsize::new(0),
            save_head_calls: AtomicUsize::new(0),
        }
    }
}

impl Store for CountingStore {
    fn save_block(&self, root: Bytes32, block: SignedBlock) -> Result<(), StorageError> {
        self.save_block_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.save_block(root, block)
    }
    fn save_state(&self, root: Bytes32, state: protocol::State) -> Result<(), StorageError> {
        self.save_state_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.save_state(root, state)
    }
    fn save_head(&self, info: HeadInfo) -> Result<(), StorageError> {
        self.save_head_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.save_head(info)
    }
    fn has_block(&self, root: &Bytes32) -> Result<bool, StorageError> {
        self.inner.has_block(root)
    }
    fn load_block(&self, root: &Bytes32) -> Result<Option<SignedBlock>, StorageError> {
        self.inner.load_block(root)
    }
    fn load_state(&self, root: &Bytes32) -> Result<Option<protocol::State>, StorageError> {
        self.inner.load_state(root)
    }
    fn load_head(&self) -> Result<Option<HeadInfo>, StorageError> {
        self.inner.load_head()
    }
}

/// Produces a slot-1 block on a fresh producer engine; the returned
/// `(signed_block, block_root)` pair is suitable for replaying through a
/// separate importer engine.
fn slot_1_block() -> (SignedBlock, Bytes32) {
    let producer = engine_at_genesis(ENGINE_VALIDATORS);
    let signed = produce_signed_block(&producer, Slot::ONE, ValidatorIndex::new(1));
    let root: Bytes32 = signed.message.hash_tree_root().into();
    (signed, root)
}

fn fresh_service() -> (Service, Arc<CountingStore>, Engine) {
    let importer = engine_at_genesis(ENGINE_VALIDATORS);
    let store = Arc::new(CountingStore::new());
    let service = Service::new(importer.clone(), Arc::clone(&store) as Arc<dyn Store>);
    (service, store, importer)
}

#[tokio::test]
async fn accepted_block_persists_to_storage() {
    let (service, store, engine) = fresh_service();
    let (signed, root) = slot_1_block();

    let outcome = service.import_block(signed.clone()).await.unwrap();
    let BlockImportResult::Accepted {
        block_root,
        head_root,
        post_state_root,
        ..
    } = outcome
    else {
        panic!("expected Accepted, got {outcome:?}");
    };
    assert_eq!(block_root, root);
    assert_eq!(head_root, engine.head());

    // Block, state, and head all persisted.
    assert!(store.has_block(&root).unwrap());
    let saved_block = store.load_block(&root).unwrap().unwrap();
    assert_eq!(saved_block.message.slot, Slot::ONE);

    let saved_state = store.load_state(&root).unwrap().unwrap();
    let saved_state_root: Bytes32 = saved_state.hash_tree_root().into();
    assert_eq!(saved_state_root, post_state_root);

    let saved_head = store.load_head().unwrap().unwrap();
    assert_eq!(saved_head.head.root, head_root);
}

#[tokio::test]
async fn duplicate_block_returns_duplicate_no_extra_persist() {
    let (service, store, _engine) = fresh_service();
    let (signed, root) = slot_1_block();

    let first = service.import_block(signed.clone()).await.unwrap();
    assert!(matches!(first, BlockImportResult::Accepted { .. }));
    assert_eq!(store.save_block_calls.load(Ordering::SeqCst), 1);

    let second = service.import_block(signed).await.unwrap();
    assert!(matches!(
        second,
        BlockImportResult::DuplicateBlock { block_root } if block_root == root
    ));
    // No additional persist on the duplicate branch.
    assert_eq!(store.save_block_calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.save_state_calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.save_head_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_parent_surfaces_outcome_without_persist() {
    let (service, store, _engine) = fresh_service();

    let bogus_parent = Bytes32::new([0xaa; 32]);
    let orphan = SignedBlock {
        message: Block {
            slot: Slot::ONE,
            proposer_index: ValidatorIndex::new(1),
            parent_root: bogus_parent,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        },
        signature: Bytes4000::new([0; 4000]),
    };
    let outcome = service.import_block(orphan).await.unwrap();
    assert!(matches!(
        outcome,
        BlockImportResult::MissingParent { parent_root, .. } if parent_root == bogus_parent
    ));
    // No storage writes; service did not loop or retry.
    assert_eq!(store.save_block_calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.save_state_calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.save_head_calls.load(Ordering::SeqCst), 0);
}
