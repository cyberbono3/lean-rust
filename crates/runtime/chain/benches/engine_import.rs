//! Wall-clock benchmark for the engine block-import hot path.
//!
//! Establishes the pre-Phase-1 reference for the import path that the
//! persistent-store milestone touches (the mutex-held `State` deep clone and
//! the three-write persist sequence). Each timed iteration imports a freshly
//! produced slot-1 block into a fresh importer engine; the produce + genesis
//! setup is excluded from the measurement via `iter_batched`.
//!
//! Run: `cargo bench -p lean-chain --bench engine_import`.

// `criterion_group!` expands to an undocumented `pub fn`; benches are not part
// of the public API surface, so the workspace `missing_docs` lint is waived here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use lean_chain::engine::test_fixtures::{
    engine_at_genesis, produce_signed_block, ENGINE_VALIDATORS,
};
use lean_chain::engine::BlockImportResult;
use protocol::{Slot, ValidatorIndex};

fn import_block_slot1(c: &mut Criterion) {
    c.bench_function("engine_import_block_slot1", |b| {
        b.iter_batched(
            || {
                // Untimed setup: produce a slot-1 block from an independent
                // producer engine, then hand a fresh importer engine the block.
                let producer = engine_at_genesis(ENGINE_VALIDATORS);
                let signed = produce_signed_block(&producer, Slot::new(1), ValidatorIndex::new(1));
                let importer = engine_at_genesis(ENGINE_VALIDATORS);
                (importer, signed)
            },
            |(importer, signed)| {
                let outcome = importer.import_block(signed);
                assert!(
                    matches!(outcome, BlockImportResult::Accepted { .. }),
                    "expected Accepted, got {outcome:?}"
                );
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, import_block_slot1);
criterion_main!(benches);
