//! Duties [`Service`] — the narrow devnet0 proposer / attester
//! scheduler.
//!
//! Loads validator assignments at [`Service::start`], spawns a single
//! worker task that drives the scheduler against the genesis-anchored
//! clock, and forwards production through the [`Chain`] port and
//! publish through the [`Publisher`] port.
//!
//! Lifecycle mirrors [`lean_sync::Loop`]: [`lean_core::Service`]
//! impl with `start` / `stop` / `status`, cancellation-via-token, and
//! best-effort drop cleanup.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use parking_lot::Mutex;
use protocol::{Slot, ValidatorIndex};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use super::config::{Config, GenesisTimeUnix};
use super::error::{DutiesError, DutiesResult};
use super::ports::{Chain, Publisher};
use super::proposer::LocalProposers;
use super::validators::ValidatorAssignments;

/// Handle to the running scheduler worker: the spawned `JoinHandle` and
/// the `CancellationToken` that triggers loop exit. Both gone after
/// `stop`.
struct RunHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
}

/// Narrow devnet0 duties service.
///
/// Construct via [`Service::new`]; supply impls of [`Chain`] (the chain
/// service in production, an in-memory fake in tests) and [`Publisher`]
/// (the `node`-level libp2p adapter in production, a mock in tests).
pub struct Service {
    config: Config,
    chain: Arc<dyn Chain>,
    publisher: Arc<dyn Publisher>,
    /// Cached `(slot_duration, vote_due_offset)` derived from
    /// `config::DEVNET_CONFIG`. Cached so the scheduler doesn't
    /// re-multiply on every iteration.
    slot_duration: Duration,
    vote_due_offset: Duration,
    /// Shared scheduler state: the run handle (when started) and the
    /// rolling publish-health counter. Wrapped in `Arc<Mutex>` so the
    /// worker task can update health without unsafe pointer tricks (and
    /// without an `Arc` cycle, since `Worker` holds only this state arc,
    /// not the whole service).
    state: Arc<Mutex<ServiceState>>,
}

/// Number of consecutive duty failures (production or publish) after
/// which [`lean_core::Service::status`] flips `Ok → Err`. Tolerating
/// `K-1` flakes keeps a single transport hiccup from paging an
/// operator while still surfacing a sustained outage.
const PUBLISH_FAILURE_THRESHOLD: u32 = 3;

#[derive(Default)]
struct ServiceState {
    run: Option<RunHandle>,
    /// Rolling publish-health counter. Surfaced via
    /// [`lean_core::Service::status`]. Reset on each `start`.
    health: PublishHealth,
}

/// Rolling record of duty-publish health, replacing the prior
/// fire-and-forget `last_err` slot (which never recorded publish
/// failures at all). A run of [`PUBLISH_FAILURE_THRESHOLD`] consecutive
/// failures flips `status()` to `Err`; any success resets the streak.
#[derive(Default, Debug)]
struct PublishHealth {
    /// Consecutive failures since the last success. Reset to 0 on any
    /// successful publish.
    consecutive_failures: u32,
    /// The most recent failure, retained for the `status()` message.
    last_error: Option<DutiesError>,
    /// Slot of the most recent failure.
    last_failure_slot: Option<u64>,
}

impl PublishHealth {
    /// Records a failed duty at `slot`, incrementing the consecutive
    /// streak and capturing the error for diagnostics.
    fn on_failure(&mut self, slot: Slot, err: DutiesError) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_error = Some(err);
        self.last_failure_slot = Some(slot.get());
    }

    /// Records a successful publish, clearing the consecutive streak.
    /// `last_error` / `last_failure_slot` are left in place — they only
    /// surface while the streak is at or past the threshold.
    fn on_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Returns `Err` once the consecutive-failure streak reaches
    /// [`PUBLISH_FAILURE_THRESHOLD`], carrying the last error and the
    /// slot it occurred on; `Ok` otherwise.
    fn status(&self) -> anyhow::Result<()> {
        if self.consecutive_failures < PUBLISH_FAILURE_THRESHOLD {
            return Ok(());
        }
        let slot = self.last_failure_slot.unwrap_or_default();
        match self.last_error.as_ref() {
            Some(err) => Err(anyhow!(
                "duties publish degraded: {} consecutive failures (last at slot {slot}): {err}",
                self.consecutive_failures,
            )),
            None => Err(anyhow!(
                "duties publish degraded: {} consecutive failures",
                self.consecutive_failures,
            )),
        }
    }
}

impl core::fmt::Debug for Service {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("Service")
            .field("config", &self.config)
            .field("slot_duration", &self.slot_duration)
            .field("vote_due_offset", &self.vote_due_offset)
            .field("running", &state.run.is_some())
            .field("consecutive_failures", &state.health.consecutive_failures)
            .finish_non_exhaustive()
    }
}

impl Service {
    /// Builds a duties service. [`Config`] is already validated at
    /// construction (parse-don't-validate), so this method is
    /// infallible at the field level — it cannot reject a malformed
    /// path / group. Validator assignments are loaded at
    /// [`Service::start`], not here, so a missing fixture file does
    /// not prevent service composition.
    #[must_use]
    pub fn new(config: Config, chain: Arc<dyn Chain>, publisher: Arc<dyn Publisher>) -> Self {
        let slot_duration_ms = config.slot_duration_ms().get();
        let slot_duration = Duration::from_millis(slot_duration_ms);
        // Integer math: slot_duration_ms * bps / 10_000. The
        // `vote_due_bps` factor comes from the compile-time
        // `DEVNET_CONFIG` const and is bounded to [0, 10_000]; at the
        // realistic `slot_duration_ms` devnet range the product stays
        // far below `u64::MAX`, so overflow is not reachable.
        let vote_due_offset =
            Duration::from_millis(slot_duration_ms * config::DEVNET_CONFIG.vote_due_bps / 10_000);
        Self {
            config,
            chain,
            publisher,
            slot_duration,
            vote_due_offset,
            state: Arc::new(Mutex::new(ServiceState::default())),
        }
    }

    /// Returns the cached `(slot_duration, vote_due_offset)` pair.
    ///
    /// Useful for tests that need to drive `tokio::time` past slot
    /// boundaries without re-deriving the constants.
    #[must_use]
    pub const fn timing(&self) -> (Duration, Duration) {
        (self.slot_duration, self.vote_due_offset)
    }

    /// Returns the validated configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl Drop for Service {
    /// Best-effort cleanup if the service is dropped without going
    /// through [`lean_core::Service::stop`]: cancel the shared token
    /// so the worker exits on its next iteration. The handle detaches;
    /// cancellation guarantees the task does not loop holding `Arc`
    /// clones of the chain / publisher.
    fn drop(&mut self) {
        if let Some(handle) = self.state.lock().run.take() {
            handle.cancel.cancel();
        }
    }
}

#[async_trait]
impl lean_core::Service for Service {
    fn name(&self) -> &'static str {
        "duties"
    }

    #[instrument(level = "info", name = "duties.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        // Reject a config that would schedule fictitious slots (epoch
        // genesis) before doing any work. `slot_duration_ms` is a
        // `NonZeroU64`, so the divide-by-zero case is already
        // unrepresentable.
        self.config.ensure_runnable()?;
        // Load assignments before flipping the running flag so a load
        // failure leaves the service stoppable and re-startable.
        let assignments = ValidatorAssignments::load(self.config.validators_path())
            .context("load validator assignments")?;
        let group = self.config.validator_group();
        // `assignments.group(...) -> Some(non_empty)` is a loader
        // invariant: `ValidatorAssignments::load` rejects empty groups
        // at parse time. Trust the type-level guarantee here.
        let local = assignments
            .group(group)
            .ok_or_else(|| DutiesError::UnknownValidatorGroup(group.to_owned()))?;
        info!(
            group = group,
            validators = local.len(),
            total = assignments.total_validators(),
            "duties validators selected",
        );

        let mut state = self.state.lock();
        if state.run.is_some() {
            return Err(DutiesError::AlreadyStarted.into());
        }
        state.health = PublishHealth::default();

        let cancel = CancellationToken::new();
        let worker = Worker {
            chain: Arc::clone(&self.chain),
            publisher: Arc::clone(&self.publisher),
            validators: local.into(),
            proposers: LocalProposers::new(local.iter().copied(), assignments.total_validators()),
            slot_duration: self.slot_duration,
            vote_due_offset: self.vote_due_offset,
            genesis: Genesis::new(self.config.genesis_time_unix()),
            cancel: cancel.clone(),
            progress: Progress::default(),
            health: HealthSink {
                state: Arc::clone(&self.state),
            },
        };
        let task = tokio::spawn(worker.run());
        state.run = Some(RunHandle { task, cancel });
        Ok(())
    }

    #[instrument(level = "info", name = "duties.stop", skip_all, err)]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(RunHandle {
            mut task,
            cancel: own_cancel,
        }) = self.state.lock().run.take()
        else {
            return Ok(());
        };
        own_cancel.cancel();

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                task.abort();
                _ = (&mut task).await;
                Err(anyhow!("duties worker did not stop within shutdown budget"))
            }
            join = &mut task => {
                join.context("duties worker task panicked")?;
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        let state = self.state.lock();
        match state.run.as_ref() {
            None => Err(anyhow!("duties service is not running")),
            Some(h) if h.task.is_finished() => {
                Err(anyhow!("duties worker task exited prematurely"))
            }
            Some(_) => state.health.status(),
        }
    }
}

/// Worker-side handle to the shared publish-health counter. Recording
/// here surfaces via [`lean_core::Service::status`] without the worker
/// holding the whole [`Service`].
struct HealthSink {
    state: Arc<Mutex<ServiceState>>,
}

impl HealthSink {
    fn record_failure(&self, slot: Slot, err: DutiesError) {
        self.state.lock().health.on_failure(slot, err);
    }

    fn record_success(&self) {
        self.state.lock().health.on_success();
    }
}

/// Wall-clock genesis anchor expressed as a `tokio::time::Instant`.
///
/// Computed once at start: maps the configured `genesis_time_unix`
/// (seconds since epoch) onto the `tokio::time` clock so the scheduler
/// can use `sleep_until` directly. `tokio::time::Instant` is monotonic;
/// the offset captured here remains stable across the worker's lifetime.
#[derive(Debug, Clone, Copy)]
struct Genesis {
    anchor: Instant,
}

impl Genesis {
    /// Maps `genesis_time_unix` onto the `tokio::time` clock.
    ///
    /// - When genesis is in the past (or zero), the anchor is `Instant::now()`
    ///   — the scheduler starts at slot 0 immediately.
    /// - When genesis is in the future, the anchor is `Instant::now() +
    ///   (genesis_wall - wall_now)`, so the worker sleeps until genesis.
    ///
    /// Past-genesis is deliberately *not* shifted backwards on the
    /// tokio clock: subtracting decades-of-seconds from
    /// `tokio::time::Instant` produces garbage on platforms where the
    /// underlying monotonic clock cannot represent the result, which is
    /// what produced the original "slot 444M" test failure. Always
    /// starting at slot 0 for past-genesis is sufficient — the
    /// scheduler is intentionally devnet-local, not a wall-clock catch-up
    /// engine.
    fn new(genesis_time_unix: GenesisTimeUnix) -> Self {
        let now_wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let target_wall = genesis_time_unix.to_duration();
        let until_target = target_wall.saturating_sub(now_wall);
        let anchor = Instant::now()
            .checked_add(until_target)
            .unwrap_or_else(Instant::now);
        Self { anchor }
    }
}

/// Per-slot progress flags tracked by the [`Worker`].
///
/// Renamed away from "state" so it doesn't collide visually with
/// [`ServiceState`] (the Service's lifecycle + last-error store).
///
/// Helper methods own the `Slot → u64` conversion so call sites read
/// as natural language (`progress.proposer_done(slot)`) instead of
/// repeating `slot.get()` + `Some(...)` boilerplate at every comparison.
#[derive(Default, Debug, Clone, Copy)]
struct Progress {
    /// `Some(slot_n)` once the proposer has been processed for `slot_n`.
    proposer_processed: Option<u64>,
    /// `Some(slot_n)` once the attester pass has run for `slot_n`.
    attester_processed: Option<u64>,
}

impl Progress {
    /// Reports whether the proposer pass has already run for `slot`.
    fn proposer_done(&self, slot: Slot) -> bool {
        self.proposer_processed == Some(slot.get())
    }

    /// Records that the proposer pass has run for `slot`.
    fn mark_proposer(&mut self, slot: Slot) {
        self.proposer_processed = Some(slot.get());
    }

    /// Reports whether the attester pass has already run for `slot`.
    fn attester_done(&self, slot: Slot) -> bool {
        self.attester_processed == Some(slot.get())
    }

    /// Records that the attester pass has run for `slot`.
    fn mark_attester(&mut self, slot: Slot) {
        self.attester_processed = Some(slot.get());
    }
}

struct Worker {
    chain: Arc<dyn Chain>,
    publisher: Arc<dyn Publisher>,
    /// Shared immutable slice of locally assigned validators.
    /// `Arc<[T]>` (vs `Arc<Vec<T>>`) keeps one heap allocation and
    /// drops the unused `capacity` field — idiomatic Rust for
    /// "shared immutable list."
    validators: Arc<[ValidatorIndex]>,
    /// O(1) proposer lookup over the local set (replaces the prior
    /// linear `is_proposer` scan).
    proposers: LocalProposers,
    slot_duration: Duration,
    vote_due_offset: Duration,
    genesis: Genesis,
    cancel: CancellationToken,
    progress: Progress,
    /// Sink for the rolling publish-health counter. Constructed at
    /// start with an `Arc` clone of the Service's state — recording a
    /// failure / success here surfaces via `Service::status`.
    health: HealthSink,
}

impl Worker {
    #[instrument(level = "debug", name = "duties.worker", skip_all)]
    async fn run(mut self) {
        loop {
            let now = Instant::now();
            let next = self.process_due(now).await;
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => break,
                () = sleep_until(next) => {}
            }
        }
    }

    async fn process_due(&mut self, now: Instant) -> Instant {
        let Some((slot, slot_start)) = self.current_slot(now) else {
            return self.genesis.anchor;
        };
        self.maybe_run_proposer(slot).await;
        let vote_due = slot_start
            .checked_add(self.vote_due_offset)
            .unwrap_or(slot_start);
        if self.maybe_run_attesters(slot, now, vote_due).await {
            return now; // re-evaluate next slot immediately
        }
        self.next_wake(slot, slot_start, vote_due)
    }

    /// Returns `Some((slot, slot_start))` when `now` is at or past the
    /// genesis anchor, `None` otherwise (pre-genesis path).
    fn current_slot(&self, now: Instant) -> Option<(Slot, Instant)> {
        if now < self.genesis.anchor {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.genesis.anchor);
        let slot_number =
            u64::try_from(elapsed.as_millis() / self.slot_duration.as_millis()).unwrap_or(u64::MAX);
        let slot = Slot::new(slot_number);
        // `Duration::saturating_mul` takes `u32`; saturate `slot_number`
        // into that range. For realistic devnet runs (slots ≈ wall-time
        // / 4s), overflow only triggers after centuries.
        let slot_multiplier = u32::try_from(slot_number).unwrap_or(u32::MAX);
        let slot_start = self
            .genesis
            .anchor
            .checked_add(self.slot_duration.saturating_mul(slot_multiplier))
            .unwrap_or(self.genesis.anchor);
        Some((slot, slot_start))
    }

    /// Runs the proposer pass at most once per slot. No-op when the
    /// pass already fired this slot.
    async fn maybe_run_proposer(&mut self, slot: Slot) {
        if self.progress.proposer_done(slot) {
            return;
        }
        self.progress.mark_proposer(slot);
        if let Err(err) = self.run_proposer(slot).await {
            warn!(slot = slot.get(), %err, "duties proposer pass failed");
            self.health.record_failure(slot, err);
        }
    }

    /// Runs the attester pass at most once per slot, gated on the
    /// `vote_due` deadline. Returns `true` when the pass fired this
    /// call so the caller can re-evaluate immediately.
    async fn maybe_run_attesters(&mut self, slot: Slot, now: Instant, vote_due: Instant) -> bool {
        if now < vote_due || self.progress.attester_done(slot) {
            return false;
        }
        self.progress.mark_attester(slot);
        self.run_attesters(slot).await;
        true
    }

    /// Next instant to wake at: the upcoming `vote_due` if attesters
    /// haven't run yet, otherwise the next slot boundary.
    fn next_wake(&self, slot: Slot, slot_start: Instant, vote_due: Instant) -> Instant {
        if self.progress.attester_done(slot) {
            slot_start
                .checked_add(self.slot_duration)
                .unwrap_or(slot_start)
        } else {
            vote_due
        }
    }

    async fn run_proposer(&self, slot: Slot) -> DutiesResult<()> {
        let Some(validator) = self.proposers.proposer_for_slot(slot) else {
            debug!(slot = slot.get(), "duties proposer slot not assigned");
            return Ok(());
        };
        let block = self.chain.produce_block(slot, validator).await?;
        match self.publisher.publish_block(block).await {
            Ok(()) => {
                info!(
                    slot = slot.get(),
                    validator = validator.get(),
                    "duties block proposed",
                );
                self.health.record_success();
            }
            Err(err) => {
                warn!(
                    slot = slot.get(),
                    validator = validator.get(),
                    %err,
                    "duties block publish failed",
                );
                self.health.record_failure(slot, err.into());
            }
        }
        Ok(())
    }

    /// Per-validator attester pass. Each step warn-logs and continues
    /// on failure — attester errors are never service-terminal, so the
    /// function is fire-and-forget rather than `Result`-returning.
    async fn run_attesters(&self, slot: Slot) {
        for validator in self.validators.iter().copied() {
            let vote = match self.chain.produce_attestation(slot, validator).await {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        slot = slot.get(),
                        validator = validator.get(),
                        %err,
                        "duties attestation production failed",
                    );
                    self.health.record_failure(slot, err.into());
                    continue;
                }
            };
            match self.publisher.publish_attestation(vote).await {
                Ok(()) => {
                    debug!(
                        slot = slot.get(),
                        validator = validator.get(),
                        "duties attestation published",
                    );
                    self.health.record_success();
                }
                Err(err) => {
                    warn!(
                        slot = slot.get(),
                        validator = validator.get(),
                        %err,
                        "duties attestation publish failed",
                    );
                    self.health.record_failure(slot, err.into());
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ports::PublishError;
    use anyhow::anyhow;
    use protocol::{SignedBlock, SignedVote};

    /// Trivial chain that produces zero-shaped blocks/votes for the
    /// construction-time tests below.
    struct NoopChain;

    #[async_trait]
    impl Chain for NoopChain {
        async fn produce_block(
            &self,
            _slot: Slot,
            _validator: ValidatorIndex,
        ) -> Result<SignedBlock, lean_chain::ChainError> {
            Ok(SignedBlock::default())
        }
        async fn produce_attestation(
            &self,
            _slot: Slot,
            _validator: ValidatorIndex,
        ) -> Result<SignedVote, lean_chain::ChainError> {
            Ok(SignedVote::default())
        }
    }

    struct NoopPublisher;

    #[async_trait]
    impl Publisher for NoopPublisher {
        async fn publish_block(&self, _b: SignedBlock) -> Result<(), PublishError> {
            Ok(())
        }
        async fn publish_attestation(&self, _v: SignedVote) -> Result<(), PublishError> {
            Err(anyhow!("test transport down").into())
        }
    }

    fn service() -> Service {
        Service::new(
            Config::default(),
            Arc::new(NoopChain),
            Arc::new(NoopPublisher),
        )
    }

    #[test]
    fn config_with_validators_path_rejects_empty() {
        // `Service::new` itself is infallible now — invalid path is
        // impossible to construct because the `Config` builder
        // re-checks the invariant.
        let err = Config::default().with_validators_path("").unwrap_err();
        assert!(
            matches!(err, DutiesError::EmptyValidatorsPath),
            "got {err:?}",
        );
    }

    #[test]
    fn timing_derives_from_devnet_config() {
        let svc = service();
        let (slot_duration, vote_due_offset) = svc.timing();
        // 4 000 ms slots, 5 000 bps → 2 000 ms vote-due offset.
        assert_eq!(slot_duration, Duration::from_millis(4_000));
        assert_eq!(vote_due_offset, Duration::from_millis(2_000));
    }

    #[tokio::test]
    async fn status_before_start_errors() {
        let svc = service();
        assert!(<Service as lean_core::Service>::status(&svc).await.is_err());
    }

    // -- PublishHealth ------------------------------------------------------

    fn sample_err() -> DutiesError {
        DutiesError::Publish(anyhow!("transport down").into())
    }

    #[test]
    fn publish_health_ok_below_threshold() {
        let mut h = PublishHealth::default();
        for _ in 0..PUBLISH_FAILURE_THRESHOLD - 1 {
            h.on_failure(Slot::new(7), sample_err());
        }
        assert!(
            h.status().is_ok(),
            "must stay Ok below K={PUBLISH_FAILURE_THRESHOLD}"
        );
    }

    #[test]
    fn publish_health_flips_exactly_on_kth_failure() {
        let mut h = PublishHealth::default();
        // K-1 failures: still Ok.
        for _ in 0..PUBLISH_FAILURE_THRESHOLD - 1 {
            h.on_failure(Slot::new(2), sample_err());
        }
        assert!(h.status().is_ok());
        // The Kth consecutive failure flips status.
        h.on_failure(Slot::new(2), sample_err());
        let err = h.status().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("slot 2"),
            "expected last_failure_slot, got {msg}"
        );
        assert!(
            msg.contains("transport down"),
            "expected last error, got {msg}"
        );
    }

    #[test]
    fn publish_health_success_resets_streak() {
        let mut h = PublishHealth::default();
        for _ in 0..PUBLISH_FAILURE_THRESHOLD {
            h.on_failure(Slot::new(1), sample_err());
        }
        assert!(h.status().is_err());
        h.on_success();
        assert!(
            h.status().is_ok(),
            "a success must clear the degraded state"
        );
    }
}
