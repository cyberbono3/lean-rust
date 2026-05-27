//! Integration tests for the duties scheduler.
//!
//! Uses `#[tokio::test(start_paused = true)]` so `tokio::time::advance`
//! drives the scheduler deterministically. The chain port and publisher
//! port are stubbed with in-memory fakes that record every call.

#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use lean_core::Service as _;
use lean_duties::{
    Chain as DutiesChain, Config as DutiesConfig, DutiesError, GenesisTimeUnix, PublishError,
    Publisher, Service as DutiesService,
};
use parking_lot::Mutex;
use protocol::{
    Block, BlockBody, BlockHeader, Checkpoint, SignedBlock, SignedVote, Slot, ValidatorIndex, Vote,
};
use tokio::time;
use tokio_util::sync::CancellationToken;
use types::{Bytes32, Bytes4000};

/// In-memory `Chain` fake. Returns deterministic `SignedBlock` /
/// `SignedVote` shaped values; records every `produce_*` call so tests
/// can assert call ordering.
#[derive(Default)]
struct FakeChain {
    produced_blocks: Mutex<Vec<(Slot, ValidatorIndex)>>,
    produced_attestations: Mutex<Vec<(Slot, ValidatorIndex)>>,
}

impl FakeChain {
    fn block_calls(&self) -> Vec<(Slot, ValidatorIndex)> {
        self.produced_blocks.lock().clone()
    }
    fn attestation_calls(&self) -> Vec<(Slot, ValidatorIndex)> {
        self.produced_attestations.lock().clone()
    }
}

#[async_trait]
impl DutiesChain for FakeChain {
    async fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedBlock, lean_chain::ChainError> {
        self.produced_blocks.lock().push((slot, validator));
        Ok(SignedBlock {
            message: Block {
                slot,
                proposer_index: validator,
                parent_root: Bytes32::zero(),
                state_root: Bytes32::zero(),
                body: BlockBody::default(),
            },
            signature: Bytes4000::new([0; 4000]),
        })
    }
    async fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedVote, lean_chain::ChainError> {
        self.produced_attestations.lock().push((slot, validator));
        let _ = BlockHeader::default(); // keep the import live
        Ok(SignedVote {
            validator_id: validator,
            message: Vote {
                slot,
                head: Checkpoint::new(Bytes32::zero(), slot),
                target: Checkpoint::new(Bytes32::zero(), slot),
                source: Checkpoint::new(Bytes32::zero(), Slot::ZERO),
            },
            signature: Bytes4000::new([0; 4000]),
        })
    }
}

/// In-memory `Publisher` fake. Captures every payload + supports an
/// "errors on next call" toggle for the publish-error tests.
#[derive(Default)]
struct MockPublisher {
    blocks: Mutex<Vec<SignedBlock>>,
    attestations: Mutex<Vec<SignedVote>>,
    fail_next: Mutex<bool>,
    fail_always: Mutex<bool>,
    block_attestations: Mutex<bool>,
}

impl MockPublisher {
    fn block_count(&self) -> usize {
        self.blocks.lock().len()
    }
    fn attestation_count(&self) -> usize {
        self.attestations.lock().len()
    }
    fn fail_once(&self) {
        *self.fail_next.lock() = true;
    }
    fn fail_all(&self) {
        *self.fail_always.lock() = true;
    }
    fn block_attestations(&self) {
        *self.block_attestations.lock() = true;
    }
    fn should_fail(&self) -> bool {
        *self.fail_always.lock() || std::mem::replace(&mut *self.fail_next.lock(), false)
    }
    fn should_block(&self) -> bool {
        *self.block_attestations.lock()
    }
}

#[async_trait]
impl Publisher for MockPublisher {
    async fn publish_block(&self, block: SignedBlock) -> Result<(), PublishError> {
        if self.should_fail() {
            return Err(anyhow!("test publish failure").into());
        }
        self.blocks.lock().push(block);
        Ok(())
    }
    async fn publish_attestation(&self, vote: SignedVote) -> Result<(), PublishError> {
        if self.should_block() {
            // Park forever (paused clock never advances): the test fires
            // a cancel mid-pass and asserts shutdown does not wait on us.
            std::future::pending::<()>().await;
        }
        if self.should_fail() {
            return Err(anyhow!("test publish failure").into());
        }
        self.attestations.lock().push(vote);
        Ok(())
    }
}

/// Repository-relative path resolved against the lean-chain crate
/// root, mirroring how production callers feed `validators_path`.
const FIXTURE_PATH: &str = "tests/fixtures/validators.yaml";
const MALFORMED_PATH: &str = "tests/fixtures/validators_malformed.yaml";

fn config(group: &str) -> DutiesConfig {
    // Genesis at the current wall-clock second. Under `start_paused`,
    // `SystemTime::now()` is still the real clock, so mapping genesis ≈
    // now onto the frozen tokio `Instant` makes the anchor land at
    // `Instant::now()` — the worker fires at slot 0 immediately, same
    // as the old `GenesisTimeUnix::new(0)`. A non-epoch value is
    // required now that `Config::ensure_runnable` rejects epoch genesis.
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

fn build(group: &str) -> (DutiesService, Arc<FakeChain>, Arc<MockPublisher>) {
    let chain = Arc::new(FakeChain::default());
    let publisher = Arc::new(MockPublisher::default());
    let service = DutiesService::new(
        config(group),
        Arc::clone(&chain) as Arc<dyn DutiesChain>,
        Arc::clone(&publisher) as Arc<dyn Publisher>,
    );
    (service, chain, publisher)
}

async fn yield_runtime() {
    // Push the scheduler past its `sleep_until` so subsequent
    // `advance()` calls actually fire the wakeup.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn proposer_publishes_block_at_slot_boundary() {
    // ream group owns indices [0, 3, 6, ..., 27]; index 3 proposes slot
    // 3 (3 % 30 = 3). We need to advance three slot durations to land
    // on a ream-owned proposer slot.
    let (service, chain, publisher) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;

    let (slot_duration, _) = service.timing();
    // Three full slots = slot 3 (the first ream-owned proposer slot).
    for _ in 0..3 {
        time::advance(slot_duration).await;
        yield_runtime().await;
    }

    let blocks = chain.block_calls();
    assert!(
        blocks
            .iter()
            .any(|(s, v)| *v == ValidatorIndex::new(3) && *s == Slot::new(3)),
        "expected validator 3 to produce slot-3 block; got {blocks:?}",
    );
    assert!(publisher.block_count() >= 1);

    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn attester_publishes_at_vote_due() {
    let (service, chain, publisher) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;

    let (_, vote_due_offset) = service.timing();
    // Half a slot puts us right at the vote-due deadline for slot 0.
    time::advance(vote_due_offset).await;
    yield_runtime().await;

    let attestations = chain.attestation_calls();
    let group_size = 10; // ream has 10 validators
    let slot_zero_count = attestations
        .iter()
        .filter(|(s, _)| *s == Slot::ZERO)
        .count();
    assert_eq!(
        slot_zero_count, group_size,
        "expected every ream validator to attest slot 0; got {attestations:?}",
    );
    assert!(publisher.attestation_count() >= group_size);

    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unknown_validator_group_is_rejected_at_start() {
    let chain = Arc::new(FakeChain::default());
    let publisher = Arc::new(MockPublisher::default());
    let service = DutiesService::new(config("does-not-exist"), chain, publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("UnknownValidatorGroup") || formatted.contains("does-not-exist"),
        "expected UnknownValidatorGroup, got {formatted}",
    );
}

#[tokio::test(start_paused = true)]
async fn malformed_yaml_is_rejected_at_start() {
    let chain = Arc::new(FakeChain::default());
    let publisher = Arc::new(MockPublisher::default());
    // Genesis must be set (non-epoch) so `start` reaches the YAML load
    // rather than short-circuiting on the genesis guard.
    let cfg = config("ream").with_validators_path(MALFORMED_PATH).unwrap();
    let service = DutiesService::new(cfg, chain, publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}").to_lowercase();
    assert!(
        formatted.contains("yaml") || formatted.contains("parse"),
        "expected YAML parse error, got {formatted}",
    );
}

#[tokio::test(start_paused = true)]
async fn epoch_genesis_is_rejected_at_start() {
    // `DutiesConfig::default()` leaves genesis at the Unix epoch; the
    // service must refuse to start rather than schedule fictitious slots.
    let chain = Arc::new(FakeChain::default());
    let publisher = Arc::new(MockPublisher::default());
    let cfg = DutiesConfig::default()
        .with_validators_path(FIXTURE_PATH)
        .unwrap()
        .with_validator_group("ream")
        .unwrap();
    let service = DutiesService::new(cfg, chain, publisher);
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("GenesisTimeUnset") || formatted.contains("genesis_time_unix"),
        "expected GenesisTimeUnset, got {formatted}",
    );
}

#[tokio::test(start_paused = true)]
async fn double_start_returns_already_started() {
    let (service, _chain, _publisher) = build("ream");
    service.start().await.unwrap();
    let err = service.start().await.unwrap_err();
    let formatted = format!("{err:?}");
    assert!(
        formatted.contains("AlreadyStarted") || formatted.contains("already started"),
        "expected AlreadyStarted, got {formatted}",
    );
    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn publisher_error_does_not_stop_scheduler() {
    let (service, _chain, publisher) = build("ream");
    publisher.fail_once();
    service.start().await.unwrap();
    yield_runtime().await;

    let (_, vote_due_offset) = service.timing();
    // First attester pass at slot 0 will swallow the failure.
    time::advance(vote_due_offset).await;
    yield_runtime().await;

    // Advance into slot 1, then through its vote-due deadline.
    let (slot_duration, _) = service.timing();
    time::advance(slot_duration).await;
    yield_runtime().await;
    time::advance(vote_due_offset).await;
    yield_runtime().await;

    // Scheduler kept running: more attestations published on the
    // second cycle than the (failed) first.
    assert!(
        publisher.attestation_count() >= 10,
        "expected slot-1 attestations to publish after failure, got {}",
        publisher.attestation_count(),
    );

    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn status_flips_to_err_after_consecutive_publish_failures() {
    // A publisher that always fails: the slot-0 attester pass publishes
    // ten attestations, all failing, which crosses the K=3 consecutive
    // threshold. status() must then report degraded publish health —
    // the old `last_err` slot never recorded publish failures at all.
    let (service, _chain, publisher) = build("ream");
    publisher.fail_all();
    service.start().await.unwrap();
    yield_runtime().await;
    // status starts Ok (no failures yet).
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

    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_inflight_attestation_duties() {
    // A publisher that parks forever on every attestation. Once the
    // slot-0 attester pass spawns its concurrent duties, they all block
    // in publish. `stop` fires the worker's cancel token, which the
    // drive loop's `select!` observes and breaks on — shutdown must not
    // wait on the stuck duties. If cancellation regressed, the worker
    // would never join and this test would hang.
    let (service, _chain, publisher) = build("ream");
    publisher.block_attestations();
    service.start().await.unwrap();
    yield_runtime().await;

    let (_, vote_due_offset) = service.timing();
    time::advance(vote_due_offset).await;
    yield_runtime().await;

    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn stop_without_start_is_noop() {
    let (service, _chain, _publisher) = build("ream");
    let cancel = CancellationToken::new();
    // Stopping a never-started service is well-defined.
    service.stop(cancel).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn status_returns_error_before_start() {
    let (service, _chain, _publisher) = build("ream");
    assert!(service.status().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn status_returns_ok_while_running() {
    let (service, _chain, _publisher) = build("ream");
    service.start().await.unwrap();
    yield_runtime().await;
    service.status().await.unwrap();
    let cancel = CancellationToken::new();
    service.stop(cancel).await.unwrap();
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
