//! Sync [`Loop`] — the devnet sync orchestrator.
//!
//! On each peer-ready event (a peer whose connect-handshake `Status` the
//! p2p side has cached) the loop reads that `Status`, compares heads, and—if
//! the peer is ahead—walks backwards from the peer's head one root at a time
//! via `BlocksByRoot` up to [`Config::max_sync_depth`], then imports the
//! recovered chain in forward order through the concrete
//! [`crate::chain::Service`].
//!
//! Per-block import errors are warn-logged and dropped: an unknown
//! parent at the deepest layer (when the cap is hit before the walk
//! finds a known block) is the expected outcome and is resolved on a
//! future peer-connect or via gossip.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use lean_wire::{BlocksByRootRequest, Status};
use parking_lot::Mutex;
use protocol::SignedBlockWithAttestation;
use ssz::HashTreeRoot;
use tokio::sync::mpsc;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, instrument, warn, Instrument, Span};
use types::Bytes32;

use crate::chain::Service as ChainService;
use crate::p2p::P2pService;

use crate::sync::config::Config;
use crate::sync::error::SyncError;
use crate::sync::peer_id::PeerId;

/// Buffer for the peer-ready event channel, decoupled from
/// `max_concurrent_peer_syncs` (the concurrent-walk cap). A connect burst can
/// enqueue many peer-ready events while the watch loop is still draining or
/// waiting on walk permits; the channel is lossy on overflow, and a dropped
/// event means that peer is not synced until it re-handshakes. Sizing it
/// generously (not to the small walk-permit cap) avoids permanently missing
/// peers under load while keeping the channel bounded — dedup + the walk
/// permit cap still bound downstream work.
const PEER_EVENT_CHANNEL_BOUND: usize = 256;

/// Handle to the running watch task: the spawned `JoinHandle`, the
/// `TaskTracker` that owns each per-peer `on_connect` task, and the
/// `CancellationToken` that triggers loop exit.
struct RunHandle {
    watch: JoinHandle<()>,
    peers: TaskTracker,
    cancel: CancellationToken,
}

/// Single-watcher sync orchestrator.
///
/// Construct with [`Loop::new`]; supply the concrete
/// [`crate::chain::Service`] and the concrete [`P2pService`]. The former
/// `Network` / `PeerEventProvider` port traits collapsed to this one
/// concrete handle: outbound RPC and connect events both come from
/// `P2pService`, which speaks base-58 `String` peer ids, so this module
/// stays `libp2p`-free.
///
/// Spawned per-peer `on_connect` tasks are owned by an internal
/// [`TaskTracker`]; [`Loop::stop`] cancels the shared token and awaits
/// the tracker under the caller-supplied shutdown budget, so peer tasks
/// always observe cancellation before the loop returns.
pub struct Loop {
    config: Config,
    chain: Arc<ChainService>,
    p2p: Arc<P2pService>,
    run: Mutex<Option<RunHandle>>,
}

impl core::fmt::Debug for Loop {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Loop")
            .field("config", &self.config)
            .field("running", &self.run.lock().is_some())
            .finish_non_exhaustive()
    }
}

impl Loop {
    /// Builds a `Loop` around the supplied ports. The configuration is
    /// type-validated at construction (`max_sync_depth` is `NonZeroUsize`).
    #[must_use]
    pub fn new(config: Config, chain: Arc<ChainService>, p2p: Arc<P2pService>) -> Self {
        Self {
            config,
            chain,
            p2p,
            run: Mutex::new(None),
        }
    }

    /// Returns the validated configuration.
    #[must_use]
    pub fn config(&self) -> Config {
        self.config
    }

    /// One-shot backfill against currently-connected peers: snapshots the
    /// connected set and runs each peer once through the status-exchange +
    /// walk-back + import path. **No-ops when no peer is connected** (the
    /// single-process case), which is what lets a lone node self-drive.
    ///
    /// Independent of [`crate::core::Service::start`]'s event-driven
    /// `watch_loop`; the consensus driver calls this once before its
    /// interval loop.
    #[instrument(level = "info", name = "sync.initial", skip_all)]
    pub async fn initial_sync(&self) {
        let peers = self.p2p.connected_peers();
        if peers.is_empty() {
            debug!("initial_sync: no connected peers; skipping");
            return;
        }
        let worker = PeerWorker {
            config: self.config,
            chain: Arc::clone(&self.chain),
            p2p: Arc::clone(&self.p2p),
            cancel: CancellationToken::new(),
        };
        for raw in peers {
            let Ok(peer) = PeerId::new(raw) else { continue };
            worker.clone().handle(peer).await;
        }
    }
}

impl Drop for Loop {
    /// Best-effort cleanup if the loop is dropped without going through
    /// [`crate::core::Service::stop`]: cancel the shared token so the
    /// watch task exits on its next iteration and the per-peer tasks
    /// observe shutdown. The handles detach; cancellation guarantees
    /// they will not loop holding `Arc` clones.
    fn drop(&mut self) {
        if let Some(handle) = self.run.get_mut().take() {
            handle.cancel.cancel();
            handle.peers.close();
        }
    }
}

#[async_trait]
impl crate::core::Service for Loop {
    fn name(&self) -> &'static str {
        "sync"
    }

    #[instrument(level = "info", name = "sync.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        let mut slot = self.run.lock();
        if slot.is_some() {
            return Err(SyncError::AlreadyStarted.into());
        }
        // Subscribe to peer-ready events over a bounded channel (sync +
        // infallible): the swarm task pushes a peer id once its handshake
        // `Status` is cached, so `status_exchange` finds it populated. A
        // full channel drops the event (bounded, lossy).
        let events = self.p2p.subscribe_connected_peers(PEER_EVENT_CHANNEL_BOUND);
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let worker = PeerWorker {
            config: self.config,
            chain: Arc::clone(&self.chain),
            p2p: Arc::clone(&self.p2p),
            cancel: cancel.clone(),
        };
        // One permit per allowed concurrent peer walk. The watch loop
        // acquires before spawning, so a flap storm backpressures here
        // instead of fanning out unbounded tasks.
        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrent_peer_syncs.get()));
        let watch = tokio::spawn(watch_loop(worker, events, tracker.clone(), semaphore));
        *slot = Some(RunHandle {
            watch,
            peers: tracker,
            cancel,
        });
        Ok(())
    }

    #[instrument(level = "info", name = "sync.stop", skip_all, err)]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(RunHandle {
            mut watch,
            peers,
            cancel: own_cancel,
        }) = self.run.lock().take()
        else {
            return Ok(());
        };
        own_cancel.cancel();
        peers.close();

        let peers_wait = peers.wait();
        tokio::pin!(peers_wait);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                watch.abort();
                _ = (&mut watch).await;
                Err(anyhow!("sync watch task did not stop within shutdown budget"))
            }
            join = &mut watch => {
                join.context("sync watch task panicked")?;
                // Watch exited cleanly; drain remaining per-peer tasks.
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(anyhow!(
                        "sync per-peer tasks did not drain within shutdown budget"
                    )),
                    () = &mut peers_wait => Ok(()),
                }
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        match self.run.lock().as_ref() {
            None => Err(SyncError::NotStarted.into()),
            Some(h) if h.watch.is_finished() => Err(SyncError::WatchExited.into()),
            Some(_) => Ok(()),
        }
    }
}

/// Drains peer-connect events until cancellation or sender close.
///
/// Per event: reap completed walks, dedup against any in-flight walk for
/// the same `PeerId` (a flap-storming peer yields exactly one walk),
/// acquire a [`Semaphore`] permit (capping concurrent walks at
/// `max_concurrent_peer_syncs`), then spawn an independent
/// [`PeerWorker::handle`] task tracked by `tracker`. The permit is held
/// for the task's lifetime; acquiring before the spawn means a flap
/// storm backpressures the event drain rather than fanning out.
#[instrument(level = "trace", name = "sync.watch", skip_all)]
async fn watch_loop(
    worker: PeerWorker,
    mut events: mpsc::Receiver<String>,
    tracker: TaskTracker,
    semaphore: Arc<Semaphore>,
) {
    let cancel = worker.cancel.clone();
    // In-flight walk per peer. A peer can re-sync only once its prior
    // walk completes (reaped below); a duplicate event while a walk is
    // running is dropped.
    let mut in_flight: HashMap<PeerId, JoinHandle<()>> = HashMap::new();
    loop {
        tokio::select! {
            // `biased`: cancellation has priority over event delivery.
            biased;
            () = cancel.cancelled() => break,
            maybe_peer = events.recv() => {
                let Some(raw) = maybe_peer else { break };
                // Wrap the base-58 id into the sync `PeerId`; an empty id is
                // skipped (the p2p source never emits one, but `PeerId::new`
                // rejects empties defensively).
                let Ok(peer) = PeerId::new(raw) else { continue };
                // Reap finished walks so their peers can sync again.
                in_flight.retain(|_, handle| !handle.is_finished());
                // Dedup: a walk for this peer is already running.
                if in_flight.contains_key(&peer) {
                    debug!(%peer, "sync walk already in flight; dropping duplicate peer event");
                    continue;
                }
                // Acquire a permit before spawning. Cancellation still
                // wins while we wait for a free permit.
                let permit = tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    permit = Arc::clone(&semaphore).acquire_owned() => match permit {
                        Ok(permit) => permit,
                        // The semaphore is never closed while the loop
                        // runs; a closed semaphore means shutdown.
                        Err(_) => break,
                    },
                };
                let worker = worker.clone();
                let key = peer.clone();
                let handle = tracker.spawn(
                    async move {
                        // Hold the permit for the walk's lifetime.
                        let _permit = permit;
                        worker.handle(peer).await;
                    }
                    .instrument(Span::current()),
                );
                in_flight.insert(key, handle);
            }
        }
    }
}

/// Per-peer worker: owns the ports, the cancellation token, and the
/// sync configuration. Cloned cheaply per spawned task (two `Arc`
/// refcount bumps + a [`CancellationToken`] clone).
#[derive(Clone)]
struct PeerWorker {
    config: Config,
    chain: Arc<ChainService>,
    p2p: Arc<P2pService>,
    cancel: CancellationToken,
}

impl PeerWorker {
    /// Handles a single peer-connect event: status exchange + walk-back.
    #[instrument(level = "debug", name = "sync.on_connect", skip_all, fields(peer = %peer))]
    async fn handle(self, peer: PeerId) {
        // A child of the shared token: parent cancellation (Loop::stop)
        // still cancels this walk, but the per-peer token also bounds
        // this walk in isolation (its RPC timeout aborts only this peer).
        let cancel = self.cancel.child_token();
        if cancel.is_cancelled() {
            return;
        }
        let Some((local_status, peer_status)) = status_exchange(&self.chain, &self.p2p, &peer)
        else {
            // The peer-ready event fires only after the handshake caches the
            // status, so reaching here means the peer disconnected between
            // the event and this read (its status was evicted). Skip; a later
            // reconnect/handshake re-fires the event.
            debug!(%peer, "peer status unavailable (peer disconnected before walk); skipping");
            return;
        };
        if !should_sync(&local_status, &peer_status) {
            debug!(
                local_head = local_status.head.slot.get(),
                peer_head = peer_status.head.slot.get(),
                "sync not needed",
            );
            return;
        }
        info!(
            local_head = local_status.head.slot.get(),
            peer_head = peer_status.head.slot.get(),
            "sync started",
        );
        // `Box::pin`: the future embeds the concrete `ChainService`
        // (no longer behind a trait object), so it exceeds the
        // large-future lint threshold; box it to keep the parent frame
        // small.
        Box::pin(self.sync_with_peer(&peer, peer_status.head.root, &cancel)).await;
    }

    /// Walks back from `start_root` then imports the recovered chain in
    /// forward order. `cancel` is this peer's child token.
    async fn sync_with_peer(&self, peer: &PeerId, start_root: Bytes32, cancel: &CancellationToken) {
        let Ok(pending) = self.walk_back(peer, start_root, cancel).await else {
            return;
        };
        // `Box::pin`: import future embeds the concrete `ChainService`;
        // box it to stay under the large-future lint threshold.
        Box::pin(import_chain(&self.chain, pending, cancel)).await;
    }

    /// Walks back from `start_root` collecting unknown ancestors up to
    /// `config.max_sync_depth`. Returns the collected blocks
    /// deepest-first; callers reverse the order to import oldest-first.
    /// On cancellation returns an empty `Vec` so the import phase
    /// becomes a no-op.
    #[instrument(
        level = "debug",
        name = "sync.walk_back",
        skip_all,
        err(Display, level = "warn")
    )]
    async fn walk_back(
        &self,
        peer: &PeerId,
        start_root: Bytes32,
        cancel: &CancellationToken,
    ) -> Result<Vec<SignedBlockWithAttestation>, SyncError> {
        // Wire the concrete chain + p2p pair behind the `WalkSource` seam,
        // then run the shared, source-agnostic traversal. The progression
        // logic (depth cap, stop-at-known, parent chaining, timeout and
        // cancellation handling) lives in `walk_back_with`, which is
        // unit-tested against an in-memory fake source.
        let source = P2pWalkSource {
            chain: self.chain.as_ref(),
            p2p: self.p2p.as_ref(),
            peer,
        };
        walk_back_with(
            &source,
            self.config.max_sync_depth.get(),
            self.config.request_timeout,
            start_root,
            cancel,
        )
        .await
    }
}

/// Outcome of fetching one hop of a [`walk_back_with`] traversal.
///
/// `Block` carries a full `SignedBlockWithAttestation` (dominated by the variable-length block-signature list).
/// The value is transient — produced and consumed one hop at a time, then
/// moved straight into the collected `Vec<SignedBlockWithAttestation>` — so boxing it would
/// only add an allocation and copy per hop without shrinking any retained
/// structure; the large-variant lint is not meaningful here.
#[allow(clippy::large_enum_variant)]
enum Hop {
    /// A validated block; the walk continues from its `parent_root`.
    Block(SignedBlockWithAttestation),
    /// The peer returned an empty response — stop the walk, keep progress.
    Stop,
    /// Abort this peer's walk and discard progress: the request timed out
    /// or the response failed shape/hash validation. Distinct from a
    /// propagated [`SyncError`]: an abort is an expected, non-fatal outcome
    /// resolved on a future connect, so the walk yields the empty backlog.
    Abort,
}

/// The two IO operations [`walk_back_with`] performs, abstracted so the
/// walk-progression logic is unit-testable against an in-memory fake
/// instead of a live chain + p2p pair.
#[async_trait]
trait WalkSource {
    /// Whether the block for `root` is already known locally.
    fn has_block(&self, root: &Bytes32) -> Result<bool, SyncError>;

    /// Fetches the single block for `root` over one `BlocksByRoot` hop,
    /// bounded by `timeout`, validating the response shape and hash.
    async fn fetch_hop(&self, root: Bytes32, timeout: Duration) -> Result<Hop, SyncError>;
}

/// Walks back from `start_root` collecting unknown ancestors up to
/// `max_depth`, deepest-first (callers reverse to import oldest-first).
/// Source-agnostic core of [`PeerWorker::walk_back`].
///
/// Stops cleanly (returns the collected backlog) at a locally-known
/// block, a zero parent root, an empty peer response, or the depth cap.
/// Aborts (returns an empty backlog) on cancellation, a storage read
/// error, a request timeout, or an invalid response. Propagates a
/// [`SyncError`] only for a transport failure surfaced by `fetch_hop`.
async fn walk_back_with<S: WalkSource + ?Sized>(
    source: &S,
    max_depth: usize,
    request_timeout: Duration,
    start_root: Bytes32,
    cancel: &CancellationToken,
) -> Result<Vec<SignedBlockWithAttestation>, SyncError> {
    let mut pending: Vec<SignedBlockWithAttestation> = Vec::with_capacity(max_depth);
    let mut next_root = start_root;

    for _ in 0..max_depth {
        if cancel.is_cancelled() {
            return Ok(Vec::new());
        }
        if next_root == Bytes32::zero() {
            break;
        }
        // Transient storage errors during the walk warn-log and abort THIS
        // peer's walk rather than propagating out (which would tear down the
        // spawned per-peer task with no diagnostic and appear to the caller
        // as silent sync death).
        match source.has_block(&next_root) {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => {
                warn!(%err, ?next_root, "sync walk_back: has_block failed, aborting peer walk");
                return Ok(Vec::new());
            }
        }
        // Bound the fetch by the per-peer cancel token: `Loop::stop` cancels
        // the parent of this child token, dropping the in-flight request so
        // shutdown drains in bounded time.
        let hop = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(Vec::new()),
            result = source.fetch_hop(next_root, request_timeout) => result?,
        };
        match hop {
            Hop::Block(block) => {
                next_root = block.message.block.parent_root;
                pending.push(block);
            }
            Hop::Stop => break,
            Hop::Abort => return Ok(Vec::new()),
        }
    }
    Ok(pending)
}

/// Production [`WalkSource`]: the concrete chain + p2p pair for one peer.
struct P2pWalkSource<'a> {
    chain: &'a ChainService,
    p2p: &'a P2pService,
    peer: &'a PeerId,
}

#[async_trait]
impl WalkSource for P2pWalkSource<'_> {
    fn has_block(&self, root: &Bytes32) -> Result<bool, SyncError> {
        Ok(self.chain.has_block(root)?)
    }

    async fn fetch_hop(&self, root: Bytes32, timeout: Duration) -> Result<Hop, SyncError> {
        // Proven infallible: `BlocksByRootRequest::new` only rejects
        // iterables longer than `MAX_REQUEST_BLOCKS`; a single-root array is
        // always within that bound.
        #[allow(clippy::expect_used)]
        let request = BlocksByRootRequest::new([root])
            .expect("single-root request is within MAX_REQUEST_BLOCKS");
        // A hung substream aborts only this walk after `timeout`.
        let response = match tokio::time::timeout(
            timeout,
            self.p2p.request_blocks_by_root(self.peer.as_str(), request),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => return Err(err.into()),
            Err(_elapsed) => {
                let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
                warn!(
                    peer = %self.peer,
                    timeout_ms,
                    "sync walk_back: BlocksByRoot request timed out, aborting peer walk"
                );
                return Ok(Hop::Abort);
            }
        };
        // Validate the peer's response shape and hash. A malicious or buggy
        // peer could otherwise return ANY block (or many blocks) and
        // redirect the walk; the engine would then ingest blocks that don't
        // match the requested root chain.
        match validate_single_root_response(response.blocks(), root) {
            Ok(Some(block)) => Ok(Hop::Block(block)),
            Ok(None) => Ok(Hop::Stop),
            Err(reason) => {
                warn!(
                    peer = %self.peer,
                    requested = ?root,
                    reason,
                    "sync walk_back: invalid BlocksByRoot response, aborting peer walk"
                );
                Ok(Hop::Abort)
            }
        }
    }
}

/// Validates a single-root `BlocksByRoot` response against `requested`.
///
/// - `Ok(Some(block))` — exactly one block whose `hash_tree_root` matches
///   `requested`; the walk continues from its parent.
/// - `Ok(None)` — empty response; the walk stops cleanly (not an error).
/// - `Err(reason)` — a protocol violation the caller must abort on: more
///   than one block for a single-root request, or a block whose hash does
///   not match the requested root (a malicious/buggy peer trying to
///   redirect the walk).
fn validate_single_root_response(
    blocks: &[SignedBlockWithAttestation],
    requested: Bytes32,
) -> Result<Option<SignedBlockWithAttestation>, &'static str> {
    match blocks {
        [] => Ok(None),
        [block] => {
            let returned: Bytes32 = block.message.block.hash_tree_root().into();
            if returned == requested {
                Ok(Some(block.clone()))
            } else {
                Err("returned block hash does not match requested root")
            }
        }
        _ => Err("more than one block for single-root request"),
    }
}

/// Reads the local status via `chain.local_status()` and `peer`'s last
/// handshaked status from the p2p cache (populated on the
/// `ConnectionEstablished` handshake), returning `(local, peer)`. `None`
/// when the peer's status has not been cached yet — the caller skips this
/// peer and retries on the next connect event. No RPC round-trip: the peer
/// status is a cache readback, and `local_status` acquires the engine lock
/// synchronously to capture live chain state under the single-Mutex model.
fn status_exchange(
    chain: &ChainService,
    p2p: &P2pService,
    peer: &PeerId,
) -> Option<(Status, Status)> {
    let local = chain.local_status();
    let peer_status = p2p.peer_status(peer.as_str())?;
    Some((local, peer_status))
}

/// Forward-order import: oldest first so each block's parent is already
/// resolved by the time the engine sees it. Per-block failures are
/// warn-logged and skipped; cancellation aborts remaining imports.
#[instrument(level = "debug", name = "sync.import_chain", skip_all)]
async fn import_chain(
    chain: &ChainService,
    blocks: Vec<SignedBlockWithAttestation>,
    cancel: &CancellationToken,
) {
    for block in blocks.into_iter().rev() {
        if cancel.is_cancelled() {
            return;
        }
        let slot = block.message.block.slot.get();
        // Sync backfill imports peer-provided blocks (hash-chained and
        // STF-validated by `walk_back`, but NOT signature-verified) through the
        // skip path. The sync trigger is peer-inducible, so this is a deliberate
        // trust boundary, not "already-canonical" history: it is safe only while
        // no live verifier is wired (the gate is inert). The ingress must be
        // closed — verify on sync, or bound the imported segment to a trusted
        // finalized checkpoint — before the live verifier is activated. Live
        // gossip stays on the verifying `import_block` path (see `chain::Service`).
        if let Err(err) = chain.import_block_synced(block).await {
            warn!(%err, slot, "import_block dropped");
        }
    }
}

fn should_sync(local: &Status, peer: &Status) -> bool {
    peer.finalized.slot > local.finalized.slot || peer.head.slot > local.head.slot
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use protocol::{
        Attestation, Block, BlockBody, BlockSignatures, BlockWithAttestation, Checkpoint, Slot,
        ValidatorIndex,
    };
    use std::collections::HashSet;

    fn status(finalized_slot: u64, head_slot: u64) -> Status {
        Status {
            finalized: Checkpoint::new(Bytes32::zero(), Slot::new(finalized_slot)),
            head: Checkpoint::new(Bytes32::zero(), Slot::new(head_slot)),
        }
    }

    /// Builds a `SignedBlockWithAttestation` whose `parent_root` is `parent`; distinct
    /// `slot` values yield distinct `hash_tree_root`s.
    fn signed_block(slot: u64, parent: Bytes32) -> SignedBlockWithAttestation {
        let block = Block {
            slot: Slot::new(slot),
            proposer_index: ValidatorIndex::new(slot),
            parent_root: parent,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };
        SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block,
                proposer_attestation: Attestation::default(),
            },
            signature: BlockSignatures::default(),
        }
    }

    fn root_of(block: &SignedBlockWithAttestation) -> Bytes32 {
        block.message.block.hash_tree_root().into()
    }

    #[test]
    fn validate_empty_response_stops_walk_cleanly() {
        assert_eq!(
            validate_single_root_response(&[], Bytes32::zero()),
            Ok(None)
        );
    }

    #[test]
    fn validate_single_matching_block_is_accepted() {
        let block = signed_block(7, Bytes32::zero());
        let root = root_of(&block);
        assert_eq!(
            validate_single_root_response(std::slice::from_ref(&block), root),
            Ok(Some(block)),
        );
    }

    #[test]
    fn validate_rejects_hash_mismatch() {
        // A peer returns a block whose hash does not match the requested
        // root — the walk must abort rather than ingest a redirected block.
        let block = signed_block(7, Bytes32::zero());
        let wrong = root_of(&signed_block(9, Bytes32::zero()));
        assert_ne!(root_of(&block), wrong);
        assert!(validate_single_root_response(std::slice::from_ref(&block), wrong).is_err());
    }

    #[test]
    fn validate_rejects_more_than_one_block() {
        let a = signed_block(1, Bytes32::zero());
        let b = signed_block(2, Bytes32::zero());
        let root = root_of(&a);
        assert!(validate_single_root_response(&[a, b], root).is_err());
    }

    #[test]
    fn should_sync_when_peer_head_ahead() {
        assert!(should_sync(&status(0, 0), &status(0, 1)));
    }

    #[test]
    fn should_sync_when_peer_finalized_ahead() {
        assert!(should_sync(&status(0, 5), &status(1, 5)));
    }

    #[test]
    fn no_sync_when_equal() {
        assert!(!should_sync(&status(1, 5), &status(1, 5)));
    }

    #[test]
    fn no_sync_when_local_ahead() {
        assert!(!should_sync(&status(1, 5), &status(1, 4)));
    }

    /// How the fake source responds to a `fetch_hop`, independent of which
    /// root is requested. `Serve` returns the chain block (or `Stop` when
    /// absent); the others exercise each abort/error branch.
    enum Fetch {
        Serve,
        Timeout,
        Invalid,
        Error,
    }

    /// In-memory [`WalkSource`]: a peer chain keyed by root, a set of
    /// locally-known roots (for `has_block`), and switches for the storage
    /// error + fetch failure branches.
    struct FakeSource {
        peer_blocks: HashMap<Bytes32, SignedBlockWithAttestation>,
        known: HashSet<Bytes32>,
        has_block_err: bool,
        fetch: Fetch,
    }

    impl FakeSource {
        fn serving(peer_blocks: HashMap<Bytes32, SignedBlockWithAttestation>) -> Self {
            Self {
                peer_blocks,
                known: HashSet::new(),
                has_block_err: false,
                fetch: Fetch::Serve,
            }
        }
    }

    #[async_trait]
    impl WalkSource for FakeSource {
        fn has_block(&self, root: &Bytes32) -> Result<bool, SyncError> {
            if self.has_block_err {
                return Err(SyncError::Network("has_block boom".into()));
            }
            Ok(self.known.contains(root))
        }

        async fn fetch_hop(&self, root: Bytes32, _timeout: Duration) -> Result<Hop, SyncError> {
            match self.fetch {
                Fetch::Timeout | Fetch::Invalid => Ok(Hop::Abort),
                Fetch::Error => Err(SyncError::Network("rpc boom".into())),
                Fetch::Serve => match self.peer_blocks.get(&root) {
                    Some(block) => Ok(Hop::Block(block.clone())),
                    None => Ok(Hop::Stop),
                },
            }
        }
    }

    /// Builds a linear chain `b1 <- b2 <- ... <- bn` (b1's parent is zero),
    /// returning the blocks oldest-first plus a root->block map for the
    /// peer source.
    fn chain(
        len: u64,
    ) -> (
        Vec<SignedBlockWithAttestation>,
        HashMap<Bytes32, SignedBlockWithAttestation>,
    ) {
        let mut blocks = Vec::new();
        let mut parent = Bytes32::zero();
        let mut map = HashMap::new();
        for slot in 1..=len {
            let block = signed_block(slot, parent);
            parent = root_of(&block);
            map.insert(root_of(&block), block.clone());
            blocks.push(block);
        }
        (blocks, map)
    }

    #[tokio::test]
    async fn walk_back_collects_all_hops_deepest_first() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let source = FakeSource::serving(map);
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("walk succeeds");
        // Deepest-first: head (slot 3), then slot 2, then slot 1.
        let slots: Vec<u64> = out.iter().map(|b| b.message.block.slot.get()).collect();
        assert_eq!(slots, vec![3, 2, 1]);
    }

    #[tokio::test]
    async fn walk_back_stops_at_known_block() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let mut source = FakeSource::serving(map);
        // The slot-2 block is already known locally; the walk stops there.
        source.known.insert(root_of(&blocks[1]));
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("walk succeeds");
        let slots: Vec<u64> = out.iter().map(|b| b.message.block.slot.get()).collect();
        assert_eq!(slots, vec![3], "walk halts at the first known ancestor");
    }

    #[tokio::test]
    async fn walk_back_caps_at_max_sync_depth() {
        let (blocks, map) = chain(5);
        let head = root_of(blocks.last().unwrap());
        let source = FakeSource::serving(map);
        let out = walk_back_with(
            &source,
            2,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("walk succeeds");
        assert_eq!(out.len(), 2, "walk stops after max_sync_depth hops");
        let slots: Vec<u64> = out.iter().map(|b| b.message.block.slot.get()).collect();
        assert_eq!(slots, vec![5, 4]);
    }

    #[tokio::test]
    async fn walk_back_stops_at_zero_root() {
        let source = FakeSource::serving(HashMap::new());
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            Bytes32::zero(),
            &CancellationToken::new(),
        )
        .await
        .expect("walk succeeds");
        assert!(out.is_empty(), "a zero start root yields no blocks");
    }

    #[tokio::test]
    async fn walk_back_stops_on_empty_response() {
        // The peer serves nothing for the requested head: clean stop, no err.
        let source = FakeSource::serving(HashMap::new());
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            root_of(&signed_block(9, Bytes32::zero())),
            &CancellationToken::new(),
        )
        .await
        .expect("walk succeeds");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn walk_back_aborts_on_has_block_error() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let mut source = FakeSource::serving(map);
        source.has_block_err = true;
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("storage error aborts to an empty backlog, not an Err");
        assert!(out.is_empty(), "a storage read error discards progress");
    }

    #[tokio::test]
    async fn walk_back_aborts_on_fetch_timeout() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let mut source = FakeSource::serving(map);
        source.fetch = Fetch::Timeout;
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("a timeout aborts to an empty backlog");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn walk_back_aborts_on_invalid_response() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let mut source = FakeSource::serving(map);
        source.fetch = Fetch::Invalid;
        let out = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await
        .expect("an invalid response aborts to an empty backlog");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn walk_back_propagates_transport_error() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let mut source = FakeSource::serving(map);
        source.fetch = Fetch::Error;
        let result = walk_back_with(
            &source,
            10,
            Duration::from_secs(1),
            head,
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(SyncError::Network(_))),
            "a transport failure propagates as SyncError, not a silent abort",
        );
    }

    #[tokio::test]
    async fn walk_back_returns_empty_when_cancelled() {
        let (blocks, map) = chain(3);
        let head = root_of(blocks.last().unwrap());
        let source = FakeSource::serving(map);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let out = walk_back_with(&source, 10, Duration::from_secs(1), head, &cancel)
            .await
            .expect("cancellation aborts to an empty backlog");
        assert!(out.is_empty(), "a pre-cancelled walk imports nothing");
    }
}
