//! Self-driving consensus loop.
//!
//! One task advances the forkchoice clock, proposes at the slot boundary,
//! attests at the vote-due interval, drains gossip, and publishes — all
//! under the single engine writer ([`ChainService`]). The driver replaces
//! the deleted per-service tick loop + duty scheduler + gossip-ingest
//! tasks: cross-service composition lives here in the `node` crate, and
//! exactly one task is spawned (no per-validator spawn), so every engine
//! mutation still serializes on the one engine mutex.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use protocol::{SignedBlock, SignedVote, Slot, ValidatorIndex};
use runtime::chain::Service as ChainService;
use runtime::core::Service;
use runtime::duties::{
    Config as DutiesConfig, GenesisTimeUnix, LocalProposers, ValidatorAssignments,
};
use runtime::p2p::{BlockReceiver, P2pService, VoteReceiver};
use runtime::sync::Loop as SyncLoop;
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

/// Intervals per slot; interval 0 is the slot boundary (propose),
/// interval `VOTE_DUE_INTERVAL` is vote-due (attest).
const INTERVALS_PER_SLOT: u64 = config::INTERVALS_PER_SLOT;

/// Wall-clock period between forkchoice intervals.
const TICK_PERIOD: Duration = Duration::from_secs(config::SECONDS_PER_INTERVAL);

/// Vote-due interval (50% of the slot); interval 2 at `IPS = 4`.
const VOTE_DUE_INTERVAL: u64 = INTERVALS_PER_SLOT / 2;

/// Consecutive proposer-slot failures (block production, or publishing a
/// produced block) after which [`ConsensusLoop::status`] reports
/// unhealthy. Mirrors the escalation the deleted duties `Service` applied
/// via `PublishHealth`: a node that keeps ticking but fails to produce or
/// publish on every slot must not report healthy forever.
const HEALTH_FAILURE_THRESHOLD: u32 = 3;

/// Rolling health signal shared between the spawned [`Runner`] (which
/// records the outcome of each proposer slot) and [`ConsensusLoop::status`]
/// (which reads it). A cheap `Arc<AtomicU32>` clone — no lock on the tick
/// hot path.
#[derive(Clone)]
struct Health {
    consecutive_failures: Arc<AtomicU32>,
}

impl Health {
    fn new() -> Self {
        Self {
            consecutive_failures: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Records a successful proposer slot (block produced and published),
    /// clearing the failure streak.
    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Records a failed proposer slot (production errored, or a produced
    /// block failed to publish), extending the failure streak.
    fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the failure streak has reached the escalation threshold.
    fn is_degraded(&self) -> bool {
        self.consecutive_failures.load(Ordering::Relaxed) >= HEALTH_FAILURE_THRESHOLD
    }
}

/// Construction-time inputs + the run handle. Mirrors the duties `Service`
/// shape: `start(&self)` builds a [`Runner`] from cloned fields and spawns
/// exactly one task — no `Arc<Self>`. The gossip receivers are taken from
/// `p2p` inside `start` (they exist only after `P2pService::start`, which
/// runs earlier in the node's start order).
pub struct ConsensusLoop {
    chain: Arc<ChainService>,
    p2p: Arc<P2pService>,
    proposers: LocalProposers,
    genesis_anchor: Instant,
    sync: Arc<SyncLoop>,
    health: Health,
    run: Mutex<Option<RunHandle>>,
}

/// Owned by the single spawned task; carries cloned handles + the per-run
/// gossip receivers. `Runner::run` consumes it, like the duties worker.
struct Runner {
    chain: Arc<ChainService>,
    p2p: Arc<P2pService>,
    proposers: LocalProposers,
    genesis_anchor: Instant,
    sync: Arc<SyncLoop>,
    health: Health,
    block_rx: BlockReceiver,
    vote_rx: VoteReceiver,
}

struct RunHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
}

impl ConsensusLoop {
    /// Builds the driver from the concrete services and the duties config.
    ///
    /// Loads the validator assignments (to build the local proposer lookup)
    /// and computes the genesis anchor on the `tokio::time` clock.
    ///
    /// # Errors
    /// - The duties config is not runnable (e.g. unset genesis).
    /// - The validator-assignment file cannot be loaded.
    /// - The configured validator group is absent from the assignment file.
    pub fn new(
        chain: Arc<ChainService>,
        p2p: Arc<P2pService>,
        sync: Arc<SyncLoop>,
        duties: &DutiesConfig,
    ) -> anyhow::Result<Self> {
        duties
            .ensure_runnable()
            .context("duties config not runnable")?;
        let assignments = ValidatorAssignments::load(duties.validators_path())
            .context("load validator assignments")?;
        let group = duties.validator_group();
        let local = assignments
            .group(group)
            .with_context(|| format!("validator group {group:?} not found"))?;
        info!(
            group,
            validators = local.len(),
            total = assignments.total_validators(),
            "consensus loop validators selected",
        );
        let proposers = LocalProposers::new(local.iter().copied(), assignments.total_validators());
        Ok(Self {
            chain,
            p2p,
            proposers,
            genesis_anchor: genesis_anchor(duties.genesis_time_unix()),
            sync,
            health: Health::new(),
            run: Mutex::new(None),
        })
    }
}

#[async_trait]
impl Service for ConsensusLoop {
    fn name(&self) -> &'static str {
        "consensus-loop"
    }

    #[instrument(level = "info", name = "consensus_loop.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        let mut run = self.run.lock();
        if run.is_some() {
            anyhow::bail!("consensus loop already started");
        }
        // Take the gossip receivers from p2p here (not at construction):
        // they are populated by `P2pService::start`, which runs earlier in
        // the node's fixed start order.
        let block_rx = self
            .p2p
            .take_block_receiver()
            .context("p2p block gossip receiver unavailable")?;
        let vote_rx = self
            .p2p
            .take_vote_receiver()
            .context("p2p vote gossip receiver unavailable")?;
        // Build the Runner from cloned handles — no `Arc<Self>`.
        let cancel = CancellationToken::new();
        let runner = Runner {
            chain: Arc::clone(&self.chain),
            p2p: Arc::clone(&self.p2p),
            proposers: self.proposers.clone(),
            genesis_anchor: self.genesis_anchor,
            sync: Arc::clone(&self.sync),
            health: self.health.clone(),
            block_rx,
            vote_rx,
        };
        // Exactly one task (single-writer invariant; no per-validator spawn).
        let task = tokio::spawn(runner.run(cancel.clone()));
        *run = Some(RunHandle { task, cancel });
        Ok(())
    }

    #[instrument(level = "info", name = "consensus_loop.stop", skip_all, err)]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(RunHandle { task, cancel: own }) = self.run.lock().take() else {
            return Ok(());
        };
        shutdown(task, own, cancel).await
    }

    async fn status(&self) -> anyhow::Result<()> {
        match self.run.lock().as_ref() {
            None => Err(anyhow::anyhow!("consensus loop not running")),
            Some(h) if h.task.is_finished() => {
                Err(anyhow::anyhow!("consensus loop task exited prematurely"))
            }
            // The loop is still ticking, but sustained proposer-slot failures
            // (block production or publish) mean it is not doing useful work;
            // surface that instead of reporting healthy forever.
            Some(_) if self.health.is_degraded() => Err(anyhow::anyhow!(
                "consensus loop degraded: {HEALTH_FAILURE_THRESHOLD} consecutive proposer-slot failures"
            )),
            Some(_) => Ok(()),
        }
    }
}

/// Stops a spawned run task: cancels its own token, then races a clean
/// join against the caller's shutdown-budget token. If the budget fires
/// first the task is aborted and an error is returned; otherwise the join
/// result is propagated (surfacing a panic as an error).
async fn shutdown(
    mut task: JoinHandle<()>,
    own: CancellationToken,
    budget: CancellationToken,
) -> anyhow::Result<()> {
    own.cancel();
    tokio::select! {
        biased;
        () = budget.cancelled() => {
            task.abort();
            let _ = (&mut task).await;
            Err(anyhow::anyhow!("consensus loop did not stop within shutdown budget"))
        }
        join = &mut task => {
            join.context("consensus loop task panicked")?;
            Ok(())
        }
    }
}

impl Runner {
    /// Runs initial sync once, then the genesis-anchored interval loop:
    /// drain gossip, dispatch by interval index, advance the engine.
    async fn run(mut self, cancel: CancellationToken) {
        // One-shot backfill before the interval loop (no-op single-process).
        self.sync.initial_sync().await;

        let mut ticker = interval_at(self.genesis_anchor + TICK_PERIOD, TICK_PERIOD);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut tick: u64 = 0;
        let mut has_proposal = false;

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    // `Box::pin` the per-tick work: these futures embed the
                    // concrete `ChainService` import/produce futures and would
                    // otherwise blow the `large_futures` lint on the enclosing
                    // `select!` (same reason the sync `Loop` boxes its walks).
                    Box::pin(self.drain_gossip()).await;
                    let slot = Slot::new(tick / INTERVALS_PER_SLOT);
                    let interval = tick % INTERVALS_PER_SLOT;
                    if interval == 0 {
                        // Reset + set for this slot from the proposer pass:
                        // `true` iff this node produced this slot's block.
                        // Sticky through the slot's remaining intervals so the
                        // interval-3 `tick_interval` (forkchoice's sole
                        // `Phase::Proposal` branch) sees the slot's true
                        // proposal outcome. A node owning only a subset of
                        // proposers signals `true` on its own proposer slots and
                        // `false` otherwise.
                        has_proposal = Box::pin(self.maybe_propose(slot)).await;
                    } else if interval == VOTE_DUE_INTERVAL {
                        Box::pin(self.run_attesters(slot, &cancel)).await;
                    }
                    if let Err(err) = self.chain.tick_interval(has_proposal).await {
                        warn!(%err, "chain tick failed; continuing");
                    }
                    tick = tick.saturating_add(1);
                }
            }
        }
    }

    /// Interval-0 proposer pass. Returns `true` iff this node published (or
    /// at least produced) a block this slot, so `has_proposal` truthfully
    /// drives forkchoice's post-proposal vote acceptance.
    async fn maybe_propose(&self, slot: Slot) -> bool {
        let Some(validator) = self.proposers.proposer_for_slot(slot) else {
            return false;
        };
        match self.chain.produce_block(slot, validator).await {
            Ok(block) => {
                // The block is already persisted by `produce_block`; a publish
                // failure is not fatal to local forkchoice, and we still count
                // it as proposed so forkchoice accepts our own post-proposal
                // votes. It does mean peers never see our block, so a publish
                // failure counts against health just like a production failure.
                match self.publish_block(&block).await {
                    Ok(()) => {
                        self.health.record_success();
                        info!(
                            slot = slot.get(),
                            validator = validator.get(),
                            "block proposed"
                        );
                    }
                    Err(err) => {
                        // Produced + persisted but never reached peers — log the
                        // failure rather than a misleading "proposed" success.
                        warn!(slot = slot.get(), %err, "block produced but publish failed");
                        self.health.record_failure();
                    }
                }
                true
            }
            Err(err) => {
                warn!(slot = slot.get(), %err, "block production failed");
                self.health.record_failure();
                false
            }
        }
    }

    /// Interval-`VOTE_DUE_INTERVAL` attester pass. Drives all local
    /// validators concurrently via `FuturesUnordered` with a per-validator
    /// timeout — not a sequential await-loop, whose wall-time would be the
    /// sum of per-validator latencies. No task spawn (futures interleave on
    /// this one task); engine mutations still serialize on the engine mutex.
    async fn run_attesters(&self, slot: Slot, cancel: &CancellationToken) {
        let budget = TICK_PERIOD;
        let mut duties = self
            .proposers
            .local()
            .map(|validator| async move {
                (
                    validator,
                    tokio::time::timeout(budget, self.attest_one(slot, validator)).await,
                )
            })
            .collect::<FuturesUnordered<_>>();
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                next = duties.next() => match next {
                    Some((v, Ok(Ok(())))) => {
                        info!(slot = slot.get(), validator = v.get(), "attested");
                    }
                    Some((v, Ok(Err(err)))) => {
                        warn!(slot = slot.get(), validator = v.get(), %err, "attest failed");
                    }
                    Some((v, Err(_elapsed))) => {
                        warn!(slot = slot.get(), validator = v.get(), "attest timed out");
                    }
                    None => break,
                },
            }
        }
    }

    /// Produces + publishes one validator's attestation. The own-vote is
    /// re-imported inside `produce_attestation`.
    async fn attest_one(&self, slot: Slot, validator: ValidatorIndex) -> anyhow::Result<()> {
        let vote = self.chain.produce_attestation(slot, validator).await?;
        self.publish_vote(&vote).await
    }

    /// Non-blocking per-tick drain of gossip-delivered blocks/votes into the
    /// chain. `try_recv` never blocks the interval ticker; import errors are
    /// warn-logged and dropped. Each sweep is bounded by the inbound gossip
    /// channel capacity (the p2p side `try_send`-drops on a full channel), so
    /// a flood cannot extend a single tick unboundedly.
    async fn drain_gossip(&mut self) {
        while let Ok(block) = self.block_rx.try_recv() {
            let slot = block.message.slot.get();
            if let Err(err) = self.chain.import_block(block).await {
                warn!(%err, slot, "gossip block import failed; continuing");
            }
        }
        while let Ok(vote) = self.vote_rx.try_recv() {
            if let Err(err) = self.chain.import_attestation(vote).await {
                warn!(%err, "gossip vote import failed; continuing");
            }
        }
    }

    /// Publishes `block` over the running gossip host.
    ///
    /// # Errors
    /// The p2p host is not running, or the publish fails.
    async fn publish_block(&self, block: &SignedBlock) -> anyhow::Result<()> {
        let host = self
            .p2p
            .host()
            .ok_or_else(|| anyhow::anyhow!("p2p host not running"))?;
        host.publish_block(block)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Publishes `vote` over the running gossip host.
    ///
    /// # Errors
    /// The p2p host is not running, or the publish fails.
    async fn publish_vote(&self, vote: &SignedVote) -> anyhow::Result<()> {
        let host = self
            .p2p
            .host()
            .ok_or_else(|| anyhow::anyhow!("p2p host not running"))?;
        host.publish_vote(vote)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }
}

/// Maps `genesis_time_unix` onto the `tokio::time` clock.
///
/// Past (or zero) genesis anchors at `Instant::now()` (slot 0 starts
/// immediately); future genesis anchors ahead so the loop waits. Past
/// genesis is deliberately not shifted backwards — subtracting large
/// durations from a monotonic `Instant` is unrepresentable on some
/// platforms; starting at slot 0 is sufficient for a devnet-local clock.
fn genesis_anchor(genesis: GenesisTimeUnix) -> Instant {
    let now_wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    anchor_from(genesis.to_duration(), now_wall, Instant::now())
}

/// Pure core of [`genesis_anchor`]: places `genesis` on the monotonic clock
/// given the current wall time (`now_wall`) and monotonic instant
/// (`now_instant`).
///
/// Past (or equal) genesis anchors at `now_instant` (slot 0 starts
/// immediately); future genesis anchors ahead by `genesis - now_wall`. An
/// anchor that would overflow the monotonic clock saturates to
/// `now_instant` rather than panicking.
fn anchor_from(genesis: Duration, now_wall: Duration, now_instant: Instant) -> Instant {
    let until_target = genesis.saturating_sub(now_wall);
    now_instant.checked_add(until_target).unwrap_or(now_instant)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // --- Health escalation (restores the deleted duties `PublishHealth`) ---

    #[test]
    fn health_starts_ok() {
        assert!(!Health::new().is_degraded());
    }

    #[test]
    fn health_degrades_after_threshold_consecutive_failures() {
        let health = Health::new();
        for _ in 0..HEALTH_FAILURE_THRESHOLD {
            assert!(!health.is_degraded(), "healthy below the threshold");
            health.record_failure();
        }
        assert!(
            health.is_degraded(),
            "the streak reaching the threshold escalates to degraded",
        );
    }

    #[test]
    fn health_success_clears_the_failure_streak() {
        let health = Health::new();
        for _ in 0..HEALTH_FAILURE_THRESHOLD - 1 {
            health.record_failure();
        }
        assert!(!health.is_degraded());
        health.record_success();
        // A reset means it takes another full run of failures to degrade.
        for _ in 0..HEALTH_FAILURE_THRESHOLD - 1 {
            health.record_failure();
        }
        assert!(
            !health.is_degraded(),
            "a single success resets the consecutive-failure count",
        );
    }

    // --- genesis_anchor pure mapping ---

    #[test]
    fn anchor_past_genesis_starts_immediately() {
        let now = Instant::now();
        // genesis (10s) precedes now_wall (100s) -> anchor at now.
        let anchor = anchor_from(Duration::from_secs(10), Duration::from_secs(100), now);
        assert_eq!(anchor, now);
    }

    #[test]
    fn anchor_future_genesis_waits_the_delta() {
        let now = Instant::now();
        // genesis (100s) is 40s after now_wall (60s) -> anchor 40s ahead.
        let anchor = anchor_from(Duration::from_secs(100), Duration::from_secs(60), now);
        assert_eq!(anchor.duration_since(now), Duration::from_secs(40));
    }

    #[test]
    fn anchor_overflow_saturates_to_now() {
        let now = Instant::now();
        // An unrepresentable far-future target saturates to now_instant
        // rather than panicking on the `Instant + Duration` overflow.
        let anchor = anchor_from(Duration::new(u64::MAX, 0), Duration::ZERO, now);
        assert_eq!(anchor, now);
    }

    // --- shutdown helper (stop()'s clean-join / abort / panic branches) ---

    #[tokio::test]
    async fn shutdown_joins_a_task_that_honors_cancellation() {
        let own = CancellationToken::new();
        let task = {
            let own = own.clone();
            tokio::spawn(async move { own.cancelled().await })
        };
        // A generous caller budget never fires: the clean-join path wins.
        shutdown(task, own, CancellationToken::new())
            .await
            .expect("a cancellation-honoring task stops cleanly");
    }

    #[tokio::test]
    async fn shutdown_aborts_a_task_that_ignores_cancellation() {
        let own = CancellationToken::new();
        // Never observes its own token: only an abort can stop it.
        let task = tokio::spawn(std::future::pending::<()>());
        let budget = CancellationToken::new();
        budget.cancel(); // pre-cancelled: the abort branch wins immediately.
        let err = shutdown(task, own, budget)
            .await
            .expect_err("a wedged task exceeds the shutdown budget");
        assert!(err.to_string().contains("shutdown budget"));
    }

    #[tokio::test]
    async fn shutdown_surfaces_a_task_panic() {
        let own = CancellationToken::new();
        let task = {
            let own = own.clone();
            tokio::spawn(async move {
                own.cancelled().await;
                panic!("boom");
            })
        };
        let err = shutdown(task, own, CancellationToken::new())
            .await
            .expect_err("a panicking task surfaces as an error, not a clean stop");
        assert!(err.to_string().contains("panicked"));
    }
}
