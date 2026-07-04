//! Integration tests for `chain::Service` lifecycle: tick loop, start /
//! stop, and status reporting.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

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

#[tokio::test(start_paused = true, flavor = "current_thread")]
async fn tick_loop_advances_engine_clock() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let service = build(engine.clone());

    let pre_slot = engine.with_store(forkchoice::Store::current_slot);
    let pre_interval = engine.with_store(forkchoice::Store::current_interval);

    service.start().await.unwrap();

    // Let the spawned task initialize its `interval_at` relative to the
    // pre-advance virtual clock; advance one period; let it fire.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(config::SECONDS_PER_INTERVAL)).await;
    tokio::task::yield_now().await;

    let post_slot = engine.with_store(forkchoice::Store::current_slot);
    let post_interval = engine.with_store(forkchoice::Store::current_interval);

    // The forkchoice clock advanced by exactly one interval — either the
    // interval index incremented (within the same slot) or the slot
    // rolled over and the interval reset to zero.
    let advanced = (post_slot == pre_slot && post_interval == pre_interval + 1)
        || (post_slot == pre_slot + 1 && pre_interval + 1 == config::INTERVALS_PER_SLOT);
    assert!(
        advanced,
        "expected one interval advance: pre=({pre_slot},{pre_interval}) post=({post_slot},{post_interval})",
    );

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(start_paused = true, flavor = "current_thread")]
async fn drop_terminates_tick_task() {
    let engine = engine_at_genesis(ENGINE_VALIDATORS);
    let service = build(engine.clone());
    service.start().await.unwrap();
    tokio::task::yield_now().await;

    let slot_before = engine.with_store(forkchoice::Store::current_slot);
    let interval_before = engine.with_store(forkchoice::Store::current_interval);

    // Drop without going through stop(): the task must be terminated, not
    // left ticking the shared engine clock.
    drop(service);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(config::SECONDS_PER_INTERVAL * 4)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        engine.with_store(forkchoice::Store::current_slot),
        slot_before,
        "tick task advanced the slot after the service was dropped",
    );
    assert_eq!(
        engine.with_store(forkchoice::Store::current_interval),
        interval_before,
        "tick task advanced the interval after the service was dropped",
    );
}

#[tokio::test]
async fn start_stop_cycle_returns_cleanly() {
    let service = build(engine_at_genesis(ENGINE_VALIDATORS));
    service.start().await.unwrap();
    service.status().await.unwrap();
    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn status_reports_unhealthy_before_start() {
    let service = build(engine_at_genesis(ENGINE_VALIDATORS));
    let err = service.status().await.unwrap_err();
    assert!(err.to_string().contains("not running"), "got: {err}");
}

#[tokio::test]
async fn status_reports_unhealthy_after_stop() {
    let service = build(engine_at_genesis(ENGINE_VALIDATORS));
    service.start().await.unwrap();
    service.stop(CancellationToken::new()).await.unwrap();

    let err = service.status().await.unwrap_err();
    assert!(err.to_string().contains("not running"), "got: {err}");
}
