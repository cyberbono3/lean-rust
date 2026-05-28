//! Wall-clock benchmark for the duties proposer lookup.
//!
//! The scheduler resolves the slot proposer on every tick. The previous
//! implementation scanned the local validator slice and called
//! `is_proposer` per entry — O(N) in the validator-set size. The
//! [`LocalProposers`] lookup is one modulo plus one hash probe, so the
//! per-slot cost must be flat across validator-set size.
//!
//! This bench times `proposer_for_slot` at devnet (N=2), mid (N=1024)
//! and a mainnet-shape set (`N=1_000_000`). The construction (building
//! the local `HashSet`) is hoisted out of the timed closure so only the
//! per-slot lookup is measured; the wall-time should be effectively
//! identical across the three N values.
//!
//! Run: `cargo bench -p lean-duties --bench local_proposer`.

// `criterion_group!` expands to an undocumented `pub fn`; benches are not part
// of the public API surface, so the workspace `missing_docs` lint is waived here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use lean_duties::LocalProposers;
use protocol::{Slot, ValidatorIndex};

fn local_proposer_lookup(c: &mut Criterion) {
    for &n in &[2u64, 1024, 1_000_000] {
        // Build the full local set once, outside the timed region.
        let proposers = LocalProposers::new((0..n).map(ValidatorIndex::new), n);
        c.bench_function(&format!("local_proposer_lookup_n{n}"), |b| {
            let mut slot = 0u64;
            b.iter(|| {
                // Step the slot so the modulo target moves across the set.
                slot = slot.wrapping_add(1);
                black_box(proposers.proposer_for_slot(black_box(Slot::new(slot))))
            });
        });
    }
}

criterion_group!(benches, local_proposer_lookup);
criterion_main!(benches);
