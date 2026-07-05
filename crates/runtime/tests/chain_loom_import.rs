//! Loom concurrency model for the engine import head-invariant.
//!
//! Scaffold, intentionally not yet a full model. All engine writes
//! (`import_block`, `produce_block`, `tick_interval`) serialize through a
//! single `parking_lot::Mutex<Store>`, so concurrent imports cannot interleave
//! mid-write: the head invariant (`finalized.slot <= head.slot`) is upheld by
//! that serialization plus the state-transition logic, not by any lock-free
//! path. A faithful loom model therefore requires instrumenting the engine to
//! use `loom::sync::Mutex` under `cfg(loom)` (loom cannot observe a
//! `parking_lot` mutex) — tracked as a separate lifecycle-hardening task rather
//! than folded into the `Arc<State>` migration.
//!
//! The `Arc<State>` capture pattern this migration introduces is memory-safe by
//! construction: a captured `Arc<State>` keeps the post-state alive even if the
//! store's map entry is later replaced, so there is no use-after-free for a
//! loom model to surface here.
//!
//! This file is gated `#![cfg(loom)]`: the default `cargo test` / clippy build
//! compiles it to an empty crate and never pulls `loom`. Run the model with:
//! `RUSTFLAGS="--cfg loom" cargo test -p runtime --test chain_loom_import`.

#![cfg(loom)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[test]
fn loom_import_head_invariant_placeholder() {
    loom::model(|| {
        // Filled in when the engine is instrumented with `loom::sync::Mutex`
        // under cfg(loom): spawn two threads each running an import, then assert
        // no interleaving reaches `finalized.slot > head.slot`.
    });
}
