//! Wall-clock benchmark + size baseline for the forkchoice vote pool.
//!
//! The pool (`latest_known_votes` / `latest_new_votes`) stores `SignedAttestation`
//! by value, and each `SignedAttestation` carries a 3116-byte XMSS signature
//! container. This bench records the reference for the mainnet footprint that a
//! later in-memory entry shrink targets: it asserts the current ~3 KiB entry
//! size and times pool population at devnet (N=2), mid (N=1024) and the spec
//! registry cap (N=4096).
//!
//! The on-wire SSZ `SignedAttestation` container is out of scope here — only the
//! in-memory pool representation may ever change.
//!
//! Run: `cargo bench -p forkchoice --bench vote_pool`.

// `criterion_group!` expands to an undocumented `pub fn`; benches are not part
// of the public API surface, so the workspace `missing_docs` lint is waived here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::HashMap;
use std::mem::size_of;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use protocol::{Attestation, AttestationData, SignedAttestation, ValidatorIndex};
use types::Signature;

/// Spec validator-registry cap, single-sourced from `config` so the bench
/// tracks the real cap instead of a drifting local copy. The mainnet vote-pool
/// footprint is bounded by this; benches must support up to (not below) this
/// many validators.
const VALIDATOR_REGISTRY_LIMIT: u64 = config::VALIDATOR_REGISTRY_LIMIT as u64;

/// Baseline floor for a vote-pool entry. An `Attestation` is 136 bytes
/// (`validator_id` 8 plus `AttestationData` 128) and a `Signature` is 3116, so
/// a `SignedAttestation` is 3252 bytes on the wire.
///
/// The assertion uses `>=` against this deliberate 3 KiB floor rather than the
/// exact width, so it tolerates struct layout and padding differences across
/// targets: the in-memory size is 3256, not 3252, once alignment padding is
/// applied. Never tighten this to `==`.
const SIGNED_ATTESTATION_BASELINE_BYTES: usize = 3072;

fn signed_attestation(validator: u64) -> SignedAttestation {
    SignedAttestation {
        message: Attestation {
            validator_id: ValidatorIndex::new(validator),
            data: AttestationData::default(),
        },
        signature: Signature::zero(),
    }
}

fn vote_pool_populate(c: &mut Criterion) {
    // Baseline footprint reference: a pool entry is a full ~3 KiB SignedAttestation
    // today. Asserting the current size keeps a later shrink measurable.
    assert!(
        size_of::<SignedAttestation>() >= SIGNED_ATTESTATION_BASELINE_BYTES,
        "expected ~3 KiB SignedAttestation baseline, got {} bytes",
        size_of::<SignedAttestation>()
    );

    for &n in &[2u64, 1024, VALIDATOR_REGISTRY_LIMIT] {
        let votes: Vec<SignedAttestation> = (0..n).map(signed_attestation).collect();
        c.bench_function(&format!("vote_pool_populate_n{n}"), |b| {
            b.iter_batched(
                || votes.clone(),
                |votes| {
                    let mut pool: HashMap<ValidatorIndex, SignedAttestation> =
                        HashMap::with_capacity(votes.len());
                    for v in votes {
                        pool.insert(v.message.validator_id, v);
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
