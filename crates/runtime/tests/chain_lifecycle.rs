//! Integration tests for `chain::Service`: the `tick_interval` engine
//! advance (which replaced the background tick loop) and the passive
//! lifecycle (start / stop / status are no-ops on an engine funnel).

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use runtime::chain::engine::test_fixtures::{engine_at_genesis, ENGINE_VALIDATORS};
use runtime::chain::engine::Engine;
use runtime::chain::Service;
use runtime::core::Service as _;
use static_assertions::assert_impl_all;
use storage::MemoryStore;
use tokio_util::sync::CancellationToken;

// Compile-time witness: `Service` must be `Send + Sync` to live inside an
// `Arc<dyn runtime::core::Service>` slot on `Node`.
assert_impl_all!(Service: Send, Sync);

fn build(engine: Engine) -> Service {
    Service::new(engine, Arc::new(MemoryStore::new()))
}

#[tokio::test(flavor = "current_thread")]
async fn tick_interval_advances_engine_clock() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    // `Engine` is `Arc`-backed, so the clone shares the store with the
    // service and observes the advance the service drives.
    let service = build(engine.clone());

    let pre_slot = engine.with_store(forkchoice::Store::current_slot);
    let pre_interval = engine.with_store(forkchoice::Store::current_interval);

    service.tick_interval(false).await.unwrap();

    let post_slot = engine.with_store(forkchoice::Store::current_slot);
    let post_interval = engine.with_store(forkchoice::Store::current_interval);

    // The forkchoice clock advanced by exactly one interval — either the
    // interval index incremented (within the same slot) or the slot rolled
    // over and the interval reset to zero.
    let advanced = (post_slot == pre_slot && post_interval == pre_interval + 1)
        || (post_slot == pre_slot + 1 && pre_interval + 1 == config::INTERVALS_PER_SLOT);
    assert!(
        advanced,
        "expected one interval advance: pre=({pre_slot},{pre_interval}) post=({post_slot},{post_interval})",
    );
}

#[tokio::test]
async fn lifecycle_is_noop_and_status_always_ok() {
    // The chain service is a passive engine funnel: no owned task, so
    // start / stop are no-ops and status is Ok regardless of lifecycle
    // state. The self-driving consensus loop owns engine advance instead.
    let service = build(engine_at_genesis(ENGINE_VALIDATORS));
    service.status().await.unwrap();
    service.start().await.unwrap();
    service.status().await.unwrap();
    service.stop(CancellationToken::new()).await.unwrap();
    service.status().await.unwrap();
}
