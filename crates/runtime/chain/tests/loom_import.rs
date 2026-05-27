//! Loom concurrency model for the engine import head-invariant.
//!
//! Placeholder scaffold. The real model — two concurrent `Service::import_block`
//! calls asserting the head invariant (`finalized.slot <= head.slot`) holds
//! across all interleavings — lands with Phase 1 (persistent-store milestone),
//! once the engine seam exposes the owned-value writes the model needs.
//!
//! This file is gated `#![cfg(loom)]`: the default `cargo test` / clippy build
//! compiles it to an empty crate and never pulls `loom`. Run the model with:
//! `RUSTFLAGS="--cfg loom" cargo test -p lean-chain --test loom_import`.

#![cfg(loom)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[test]
fn loom_import_head_invariant_placeholder() {
    loom::model(|| {
        // Phase 1 fills this in: spawn two threads each running an import,
        // then assert no interleaving reaches `finalized.slot > head.slot`.
    });
}
