//! Wall-clock benchmark + size baseline for the forkchoice vote pool.
//!
//! The pool (`latest_known_votes` / `latest_new_votes`) stores `SignedVote`
//! by value, and each `SignedVote` carries a 4000-byte XMSS signature
//! placeholder. This bench records the pre-change reference for the mainnet
//! footprint that a later in-memory entry shrink targets: it asserts the
//! current ~4 KiB entry size and times pool population at devnet (N=2),
//! mid (N=1024) and the spec registry cap (N=4096).
//!
//! The on-wire SSZ `SignedVote` container is out of scope here — only the
//! in-memory pool representation may ever change.
//!
//! Run: `cargo bench -p forkchoice --bench vote_pool`.

// Retained construction sites for the deprecated `Bytes4000` placeholder.
// Scoped to this file so unrelated deprecations elsewhere in the crate are
// still surfaced; removed when this file's last site moves to `Signature`.
#![allow(deprecated)]
// `criterion_group!` expands to an undocumented `pub fn`; benches are not part
// of the public API surface, so the workspace `missing_docs` lint is waived here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::HashMap;
use std::mem::size_of;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use protocol::{SignedVote, ValidatorIndex, Vote};
use types::Bytes4000;

/// Spec validator-registry cap (`VALIDATOR_REGISTRY_LIMIT = 2^12`) from
/// `leanSpec` `chain/config.py`. The mainnet vote-pool footprint is bounded by
/// this; benches must support up to (not below) this many validators.
const VALIDATOR_REGISTRY_LIMIT: u64 = 4096;

/// Pre-change baseline floor for a vote-pool entry: `validator_id` (8) +
/// `Vote` (~128) + `Bytes4000` (4000) ≈ 4136 bytes. The assertion uses `>=`
/// against this deliberate 4 KiB floor (not the exact 4136) so it tolerates
/// struct layout / padding differences across targets.
const SIGNED_VOTE_BASELINE_BYTES: usize = 4096;

fn signed_vote(validator: u64) -> SignedVote {
    SignedVote {
        validator_id: ValidatorIndex::new(validator),
        message: Vote::default(),
        signature: Bytes4000::new([0u8; 4000]),
    }
}

fn vote_pool_populate(c: &mut Criterion) {
    // Baseline footprint reference: a pool entry is a full ~4 KiB SignedVote
    // today. Asserting the pre-change size keeps a later shrink measurable.
    assert!(
        size_of::<SignedVote>() >= SIGNED_VOTE_BASELINE_BYTES,
        "expected ~4 KiB SignedVote baseline, got {} bytes",
        size_of::<SignedVote>()
    );

    for &n in &[2u64, 1024, VALIDATOR_REGISTRY_LIMIT] {
        let votes: Vec<SignedVote> = (0..n).map(signed_vote).collect();
        c.bench_function(&format!("vote_pool_populate_n{n}"), |b| {
            b.iter_batched(
                || votes.clone(),
                |votes| {
                    let mut pool: HashMap<ValidatorIndex, SignedVote> =
                        HashMap::with_capacity(votes.len());
                    for v in votes {
                        pool.insert(v.validator_id, v);
                    }
                    pool
                },
                BatchSize::SmallInput,
            );
        });
    }
}

criterion_group!(benches, vote_pool_populate);
criterion_main!(benches);
