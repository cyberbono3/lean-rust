//! Integration tests for the duties scheduler.
//!
//! Uses `#[tokio::test(start_paused = true)]` so `tokio::time::advance`
//! drives the scheduler deterministically. The `Chain` / `Publisher`
//! port traits were collapsed to concrete types, so these tests build a
//! real genesis-fixture [`runtime::chain::Service`] and a [`Publisher`] over
//! a constructed-but-not-started p2p host: every publish surfaces "host
//! is not running", which the scheduler tolerates by folding the failure
//! into its publish-health counter. Assertions therefore cover config
//! rejection, lifecycle, and publish-health degradation rather than the
//! former mock call-count checks.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use runtime::core::Service as _;
use runtime::duties::{
    Config as DutiesConfig, DutiesError, GenesisTimeUnix, Publisher, Service as DutiesService,
};
use runtime::p2p::{DevnetHost, HostOptions};
use storage::{MemoryStore, Store};
use tempfile::TempDir;
use tokio::time;
use tokio_util::sync::CancellationToken;

/// Repository-relative paths resolved against the duties crate root,
/// mirroring how production callers feed `validators_path`.
const FIXTURE_PATH: &str = "tests/duties_fixtures/validators.yaml";
const MALFORMED_PATH: &str = "tests/duties_fixtures/validators_malformed.yaml";

fn config(group: &str) -> DutiesConfig {
    // Genesis at the current wall-clock second. Under `start_paused`,
    // `SystemTime::now()` is still the real clock, so mapping genesis ≈
    // now onto the frozen tokio `Instant` makes the anchor land at
    // `Instant::now()` — the worker fires at slot 0 immediately. A
    // non-epoch value is required now that `Config::ensure_runnable`
    // rejects epoch genesis.
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    DutiesConfig::default()
        .with_validators_path(FIXTURE_PATH)
        .unwrap()
        .with_validator_group(group)
        .unwrap()
        .with_genesis_time_unix(GenesisTimeUnix::new(now_unix))
}

/// Genesis-fixture chain service backed by an in-memory store.
fn chain_service() -> Arc<runtime::chain::Service> {
    let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
    let engine = runtime::chain::engine::Engine::from_anchor(state, block).unwrap();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    Arc::new(runtime::chain::Service::new(engine, store))
}

/// Concrete publisher over a constructed (not started) p2p host: every
/// publish fails with "host is not running", exercising the scheduler's
/// tolerant publish-failure path. The `TempDir` backs the identity file
/// and must outlive the publisher.
fn publisher() -> (TempDir, Arc<Publisher>) {
    let dir = tempfile::tempdir().unwrap();
    let options = HostOptions::try_new(
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "test/0.1.0",
        &dir.path().join("id"),
        None,
    )
    .unwrap();
    let p2p = Arc::new(DevnetHost::build(options).unwrap());
    (dir, Arc::new(Publisher::new(p2p)))
}

/// Builds a concrete duties service over a fixture chain + non-started
/// publisher. The returned `TempDir` keeps the p2p identity file alive.
fn build(group: &str) -> (DutiesService, TempDir) {
    let (dir, publisher) = publisher();
    let service = DutiesService::new(config(group), chain_service(), publisher);
    (service, dir)
}

async fn yield_runtime() {
    // Push the scheduler past its `sleep_until` so subsequent
    // `advance()` calls actually fire the wakeup.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_validator_group_is_rejected_at_start() {
    let (dir, publisher) = publisher();
    let service = DutiesService::new(config("does-not-exist"), chain_service(), publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("UnknownValidatorGroup") || formatted.contains("does-not-exist"),
        "expected UnknownValidatorGroup, got {formatted}",
    );
    drop(dir);
}

#[tokio::test(start_paused = true)]
async fn malformed_yaml_is_rejected_at_start() {
    // Genesis must be set (non-epoch) so `start` reaches the YAML load
    // rather than short-circuiting on the genesis guard.
    let (dir, publisher) = publisher();
    let cfg = config("ream").with_validators_path(MALFORMED_PATH).unwrap();
    let service = DutiesService::new(cfg, chain_service(), publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}").to_lowercase();
    assert!(
        formatted.contains("yaml") || formatted.contains("parse"),
        "expected YAML parse error, got {formatted}",
    );
    drop(dir);
}

#[tokio::test(start_paused = true)]
async fn epoch_genesis_is_rejected_at_start() {
    // `DutiesConfig::default()` leaves genesis at the Unix epoch; the
    // service must refuse to start rather than schedule fictitious slots.
    let (dir, publisher) = publisher();
    let cfg = DutiesConfig::default()
        .with_validators_path(FIXTURE_PATH)
        .unwrap()
        .with_validator_group("ream")
        .unwrap();
    let service = DutiesService::new(cfg, chain_service(), publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("GenesisTimeUnset") || formatted.contains("genesis_time_unix"),
        "expected GenesisTimeUnset, got {formatted}",
    );
    drop(dir);
}

#[tokio::test(start_paused = true)]
async fn double_start_returns_already_started() {
    let (service, _dir) = build("ream");
    service.start().await.unwrap();
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("AlreadyStarted") || formatted.contains("already started"),
        "expected AlreadyStarted, got {formatted}",
    );
    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn publish_failures_degrade_status_but_keep_scheduler_running() {
    // With a non-started publisher every publish fails; the slot-0
    // attester pass publishes for all ream validators, crossing the K=3
    // consecutive-failure threshold. status() must flip to degraded
    // (not "task exited"), and the service must still stop cleanly.
    let (service, _dir) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;
    // status starts Ok (below the failure threshold).
    service.status().await.unwrap();

    let (_, vote_due_offset) = service.timing();
    time::advance(vote_due_offset).await;
    yield_runtime().await;

    let err = service
        .status()
        .await
        .expect_err("publish failures must degrade status");
    assert!(
        err.to_string().contains("publish degraded"),
        "expected degraded publish status, got {err}",
    );

    // Degraded, but the worker is alive and shutdown is clean.
    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn scheduler_runs_across_slot_boundaries_without_panicking() {
    // Drive several slot durations: proposer + attester passes fire and
    // fail-to-publish each slot, but the worker never wedges or panics.
    let (service, _dir) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;

    let (slot_duration, _) = service.timing();
    for _ in 0..3 {
        time::advance(slot_duration).await;
        yield_runtime().await;
    }

    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stop_without_start_is_noop() {
    let (service, _dir) = build("ream");
    // Stopping a never-started service is well-defined.
    service.stop(CancellationToken::new()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn status_returns_error_before_start() {
    let (service, _dir) = build("ream");
    assert!(service.status().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn status_returns_ok_while_running() {
    let (service, _dir) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;
    // A single slot-0 proposer failure is below the degradation
    // threshold, so status is still Ok immediately after start.
    service.status().await.unwrap();
    service.stop(CancellationToken::new()).await.unwrap();
}

#[test]
fn empty_path_rejected_by_config_builder() {
    // Invalid path can no longer reach `Service::new` — the builder
    // itself returns `Err` (parse-don't-validate).
    let err = DutiesConfig::default()
        .with_validators_path("")
        .unwrap_err();
    assert!(
        matches!(err, DutiesError::EmptyValidatorsPath),
        "got {err:?}",
    );
}
