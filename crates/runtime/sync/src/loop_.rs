//! Sync [`Loop`] — the devnet sync orchestrator.
//!
//! On each outbound peer-connect event the loop sends a `Status` RPC,
//! compares heads, and—if the peer is ahead—walks backwards from the
//! peer's head one root at a time via `BlocksByRoot` up to
//! [`Config::max_sync_depth`], then imports the recovered chain in
//! forward order through the [`Chain`] port.
//!
//! Per-block import errors are warn-logged and dropped: an unknown
//! parent at the deepest layer (when the cap is hit before the walk
//! finds a known block) is the expected outcome and is resolved on a
//! future peer-connect or via gossip.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use lean_wire::{BlocksByRootRequest, Status};
use parking_lot::Mutex;
use protocol::SignedBlock;
use ssz::HashTreeRoot;
use tokio::sync::mpsc;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, instrument, warn, Instrument, Span};
use types::Bytes32;

use crate::config::Config;
use crate::error::SyncError;
use crate::peer_id::PeerId;
use crate::ports::{Chain, Network, PeerEventProvider};

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
/// Construct with [`Loop::new`]; supply impls of [`Chain`], [`Network`],
/// and [`PeerEventProvider`] (the chain port is satisfied by
/// `Arc<crate::Service>` in production; tests use in-memory fakes).
///
/// Spawned per-peer `on_connect` tasks are owned by an internal
/// [`TaskTracker`]; [`Loop::stop`] cancels the shared token and awaits
/// the tracker under the caller-supplied shutdown budget, so peer tasks
/// always observe cancellation before the loop returns.
pub struct Loop {
    config: Config,
    chain: Arc<dyn Chain>,
    network: Arc<dyn Network>,
    peers: Arc<dyn PeerEventProvider>,
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
    pub fn new(
        config: Config,
        chain: Arc<dyn Chain>,
        network: Arc<dyn Network>,
        peers: Arc<dyn PeerEventProvider>,
    ) -> Self {
        Self {
            config,
            chain,
            network,
            peers,
            run: Mutex::new(None),
        }
    }

    /// Returns the validated configuration.
    #[must_use]
    pub fn config(&self) -> Config {
        self.config
    }
}

impl Drop for Loop {
    /// Best-effort cleanup if the loop is dropped without going through
    /// [`lean_core::Service::stop`]: cancel the shared token so the
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
impl lean_core::Service for Loop {
    fn name(&self) -> &'static str {
        "sync"
    }

    #[instrument(level = "info", name = "sync.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        // Subscribe first — a subscription failure must not flip the
        // running flag, so `start` stays idempotent against retries.
        let events = self
            .peers
            .subscribe_outbound_connected_peers()
            .await
            .context("subscribe outbound connected peers")?;

        let mut slot = self.run.lock();
        if slot.is_some() {
            return Err(SyncError::AlreadyStarted.into());
        }
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let worker = PeerWorker {
            config: self.config,
            chain: Arc::clone(&self.chain),
            network: Arc::clone(&self.network),
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
    mut events: mpsc::Receiver<PeerId>,
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
                let Some(peer) = maybe_peer else { break };
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
    chain: Arc<dyn Chain>,
    network: Arc<dyn Network>,
    cancel: CancellationToken,
}

impl PeerWorker {
    /// Handles a single peer-connect event: status exchange + walk-back.
    #[instrument(level = "debug", name = "sync.on_connect", skip_all, fields(peer = %peer))]
    async fn handle(self, peer: PeerId) {
        if self.cancel.is_cancelled() {
            return;
        }
        let Ok((local_status, peer_status)) =
            status_exchange(&*self.chain, &*self.network, &peer).await
        else {
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
        self.sync_with_peer(&peer, peer_status.head.root).await;
    }

    /// Walks back from `start_root` then imports the recovered chain in
    /// forward order.
    async fn sync_with_peer(&self, peer: &PeerId, start_root: Bytes32) {
        let Ok(pending) = self.walk_back(peer, start_root).await else {
            return;
        };
        import_chain(&*self.chain, pending, &self.cancel).await;
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
    ) -> Result<Vec<SignedBlock>, SyncError> {
        let max_depth = self.config.max_sync_depth.get();
        let mut pending: Vec<SignedBlock> = Vec::with_capacity(max_depth);
        let mut next_root = start_root;

        for _ in 0..max_depth {
            if self.cancel.is_cancelled() {
                return Ok(Vec::new());
            }
            if next_root == Bytes32::zero() {
                break;
            }
            // Transient storage errors during the walk warn-log and abort
            // THIS peer's walk rather than propagating out (which would
            // tear down the spawned per-peer task with no diagnostic and
            // appear to the caller as silent sync death).
            match self.chain.has_block(next_root).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(err) => {
                    warn!(%err, ?next_root, "sync walk_back: chain.has_block failed, aborting peer walk");
                    return Ok(Vec::new());
                }
            }
            // Proven infallible: `BlocksByRootRequest::new` only rejects
            // iterables longer than `MAX_REQUEST_BLOCKS`; a single-root
            // array is always within that bound.
            #[allow(clippy::expect_used)]
            let request = BlocksByRootRequest::new([next_root])
                .expect("single-root request is within MAX_REQUEST_BLOCKS");
            let response = self.network.request_blocks_by_root(peer, request).await?;
            let blocks = response.blocks();
            // Validate the peer's response shape and hash. A malicious or
            // buggy peer could otherwise return ANY block (or many blocks)
            // and redirect the walk; the engine would then ingest blocks
            // that don't match the requested root chain.
            if blocks.is_empty() {
                break;
            }
            if blocks.len() > 1 {
                warn!(
                    got = blocks.len(),
                    ?peer,
                    "sync walk_back: peer returned >1 block for single-root request"
                );
                return Ok(Vec::new());
            }
            let block = blocks[0].clone();
            let returned_root: Bytes32 = block.message.hash_tree_root().into();
            if returned_root != next_root {
                warn!(
                    requested = ?next_root,
                    returned = ?returned_root,
                    ?peer,
                    "sync walk_back: peer returned block whose hash does not match requested root"
                );
                return Ok(Vec::new());
            }
            next_root = block.message.parent_root;
            pending.push(block);
        }
        Ok(pending)
    }
}

/// Exchanges `Status` with `peer`: reads the local status and sends it
/// over, returning `(local, peer_reply)`.
#[instrument(
    level = "debug",
    name = "sync.status_exchange",
    skip_all,
    err(Display, level = "warn")
)]
async fn status_exchange(
    chain: &dyn Chain,
    network: &dyn Network,
    peer: &PeerId,
) -> Result<(Status, Status), SyncError> {
    let local = chain.local_status().await?;
    let peer_status = network.send_status(peer, local).await?;
    Ok((local, peer_status))
}

/// Forward-order import: oldest first so each block's parent is already
/// resolved by the time the engine sees it. Per-block failures are
/// warn-logged and skipped; cancellation aborts remaining imports.
#[instrument(level = "debug", name = "sync.import_chain", skip_all)]
async fn import_chain(chain: &dyn Chain, blocks: Vec<SignedBlock>, cancel: &CancellationToken) {
    for block in blocks.into_iter().rev() {
        if cancel.is_cancelled() {
            return;
        }
        let slot = block.message.slot.get();
        if let Err(err) = chain.import_block(block).await {
            warn!(%err, slot, "import_block dropped");
        }
    }
}

fn should_sync(local: &Status, peer: &Status) -> bool {
    peer.finalized.slot > local.finalized.slot || peer.head.slot > local.head.slot
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Checkpoint;
    use protocol::Slot;

    fn status(finalized_slot: u64, head_slot: u64) -> Status {
        Status {
            finalized: Checkpoint::new(Bytes32::zero(), Slot::new(finalized_slot)),
            head: Checkpoint::new(Bytes32::zero(), Slot::new(head_slot)),
        }
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
}
