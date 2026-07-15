//! Devnet composition entry point.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use protocol::{Block, Checkpoint, SignedBlock, Slot, State};
use runtime::api::{HttpService, MetricsService, Recorder};
use runtime::chain::{ChainMetrics, Service as ChainService};
use runtime::core::{Node, NodeConfig};
use runtime::p2p::{DevnetHost, HostOptions, RpcProvider};
use runtime::sync::{Config as SyncConfig, Loop as SyncLoop};
use storage::{HeadInfo, MemoryStore, RedbStore, Store};
use types::Bytes32;
// Retained construction sites for the deprecated `Bytes4000` placeholder; move
// to `Signature` with the container refactor. Scoped to the import, the one
// production site (`new_devnet`), and the test module, so unrelated deprecations
// in the rest of this file are still surfaced. `expect` rather than `allow`:
// once the sites move, the unfulfilled expectation fails the build.
#[expect(deprecated)]
use types::Bytes4000;

use crate::consensus_loop::ConsensusLoop;

/// Result type returned by node composition.
pub type Result<T> = anyhow::Result<T>;

/// Selects the persistence backend for the devnet node.
#[derive(Debug, Clone)]
pub enum StorageKind {
    /// In-memory, non-durable (devnet default — fast, ephemeral).
    Memory,
    /// Durable embedded key-value store rooted at the given path.
    Persistent(PathBuf),
}

impl Default for StorageKind {
    fn default() -> Self {
        Self::Memory
    }
}

/// Devnet service wiring inputs.
///
/// Existing runtime crates own validation for their domain-specific
/// options. The node config keeps only the values needed to assemble the
/// concrete service graph.
#[derive(Debug)]
#[must_use]
pub struct Config {
    /// Runtime lifecycle configuration.
    pub node: NodeConfig,
    /// libp2p host options.
    pub p2p: HostOptions,
    /// Validator duty scheduler options.
    pub duties: runtime::duties::Config,
    /// HTTP API bind address.
    pub http_addr: SocketAddr,
    /// Prometheus metrics bind address.
    pub metrics_addr: SocketAddr,
    /// Trusted genesis state used to anchor the engine.
    pub genesis_state: State,
    /// Trusted genesis block used to anchor the engine.
    pub genesis_block: Block,
    /// Persistence backend selector. Defaults to in-memory.
    pub storage: StorageKind,
}

/// Builds a devnet [`Node`] with concrete runtime services.
///
/// The composition is a flat wiring list: chain (a passive engine funnel),
/// p2p, the sync [`Loop`](runtime::sync::Loop) over the concrete p2p handle,
/// and the self-driving [`ConsensusLoop`] (in the duties slot) that owns the
/// interval loop — engine advance, propose, attest, gossip drain, and
/// publish. No workaround services (no separate tick loop, duty scheduler,
/// or gossip-ingest task).
///
/// # Errors
///
/// Returns an error if the engine rejects the genesis anchor, p2p host
/// construction fails, or the consensus loop cannot load its validators.
#[expect(deprecated)]
pub fn new_devnet(config: Config) -> Result<Node> {
    let Config {
        node,
        p2p: p2p_options,
        duties,
        http_addr,
        metrics_addr,
        genesis_state,
        genesis_block,
        storage,
    } = config;

    let store = build_store(&storage)?;

    // Build the metrics recorder and register the chain-tick trigger histograms
    // before the engine so their handles can be injected into it. Chain-state
    // gauges are registered later (they need the built `ChainService`).
    let mut recorder = Recorder::new();
    let chain_metrics = register_chain_histograms(&mut recorder)?;

    // Restart-continue: if the store already holds a head (persistent backend,
    // prior run), re-anchor the engine there and skip re-persisting the anchor.
    // Otherwise anchor at genesis and seed the store as before.
    let engine = if let Some((head_root, state, block)) = resume_anchor(store.as_ref())? {
        // Resume: trust the persisted head as its own justified+finalized anchor
        // so the fork-choice head walk starts from a tracked root. Plain
        // `from_anchor` would seed justified from the state (an ancestor absent
        // from the anchor-only block map) and break the first head recompute.
        let engine = runtime::chain::engine::Engine::from_trusted_head(state, block)?;
        // Defense-in-depth: `from_trusted_head` re-derives the anchor root from
        // the block, so a corrupt/tampered store whose head-keyed block roots
        // elsewhere would silently anchor at a different block than `load_head`
        // reported. Refuse to resume from such an inconsistent on-disk view.
        anyhow::ensure!(
            engine.head() == head_root,
            "persisted head block does not root to the recorded head; refusing to resume from an inconsistent store",
        );
        engine
    } else {
        let anchor_slot = genesis_block.slot;
        let anchor_state = genesis_state.clone();
        let signed_anchor = SignedBlock {
            message: genesis_block.clone(),
            signature: Bytes4000::default(),
        };
        let engine = runtime::chain::engine::Engine::from_anchor(genesis_state, genesis_block)?;
        let anchor_root = engine.head();
        let finalized = engine.latest_finalized();
        persist_anchor(
            store.as_ref(),
            anchor_root,
            anchor_slot,
            finalized,
            signed_anchor,
            anchor_state,
        )?;
        engine
    };
    let chain = Arc::new(ChainService::new(
        engine.with_metrics(chain_metrics),
        Arc::clone(&store),
    ));

    let rpc_provider = Arc::new(RpcProvider::chain(Arc::clone(&chain), Arc::clone(&store)));
    let p2p = Arc::new(DevnetHost::build_with_provider(p2p_options, rpc_provider)?);

    // Sync `Loop` over the concrete p2p handle (former Network /
    // PeerEventProvider ports collapsed). Runs as its own lifecycle service
    // (event-driven `watch_loop`); the driver additionally calls its
    // `initial_sync` once at startup — both are idempotent.
    let sync = Arc::new(SyncLoop::new(
        SyncConfig::default(),
        Arc::clone(&chain),
        Arc::clone(&p2p),
    ));

    // Self-driving consensus loop in the duties slot: it owns engine advance,
    // propose, attest, gossip drain, and publish.
    let driver = Arc::new(ConsensusLoop::new(
        Arc::clone(&chain),
        Arc::clone(&p2p),
        Arc::clone(&sync),
        &duties,
    )?);

    let http = Arc::new(HttpService::new(Arc::clone(&store), http_addr));
    register_chain_gauges(&mut recorder, &chain);
    let metrics = Arc::new(MetricsService::new(metrics_addr, recorder.freeze()?));

    Ok(Node::new(node)
        .with_chain(chain)
        .with_p2p(p2p)
        .with_sync(sync)
        .with_duties(driver)
        .with_http(http)
        .with_metrics(metrics))
}

/// Registers chain-state gauges. Each closure captures a cloned
/// `Arc<ChainService>` and reads the engine on demand via `snapshot()` per
/// scrape. The read acquires the engine lock, so a scrape serializes behind
/// any in-progress writer for that writer's lock hold — acceptable because a
/// scrape has no deadline. Closes the fixture §8 gap where `/metrics` exposed
/// only the two baseline process gauges.
///
/// A connected-peer gauge is intentionally not wired here: the p2p host
/// exposes no synchronous connected-peer count today, so that gauge is
/// deferred to a p2p-touching change that adds the counter.
///
/// Each gauge samples its own `snapshot()`, so one scrape reads the three
/// slots across three independent engine-lock acquisitions rather than one
/// coherent tuple. Reintroducing a per-scrape shared snapshot would re-add
/// the derived cache this refactor deletes, so it is deliberately not done:
/// the three slots are monotonic and the ordering invariant
/// `finalized <= justified <= current` holds under any interleaving, so a
/// torn read never exports an inconsistent ordering.
fn register_chain_gauges(recorder: &mut Recorder, chain: &Arc<ChainService>) {
    let slot_src = Arc::clone(chain);
    recorder.gauge(
        "lean_chain_slot",
        "Current forkchoice slot (clock).",
        move || slot_src.snapshot().current_slot,
    );

    let justified_src = Arc::clone(chain);
    recorder.gauge(
        "lean_chain_justified_slot",
        "Slot of the latest justified checkpoint.",
        move || justified_src.snapshot().latest_justified.slot.get(),
    );

    let finalized_src = Arc::clone(chain);
    recorder.gauge(
        "lean_chain_finalized_slot",
        "Slot of the latest finalized checkpoint.",
        move || finalized_src.snapshot().latest_finalized.slot.get(),
    );
}

/// Latency buckets (seconds), coarse to 4 s. The per-block `State`-clone trigger
/// buckets to 4 s; the fork-choice histogram shares the scale.
const LATENCY_BUCKETS_SECONDS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0,
];

/// Registers the deferred-performance trigger histograms and returns the live
/// handle set to inject into the engine. Mirror of [`register_chain_gauges`]:
/// the metric names and buckets are owned here at the composition root, not in
/// the runtime chain layer, so the runtime metrics registry stays decoupled
/// from `runtime::chain`.
///
/// Returns [`Result`] (like [`new_devnet`]) so the metrics error from
/// `recorder.histogram(...)?` converts at the `?` boundary — the node crate
/// names no `prometheus` item and stays free of that dependency.
///
/// # Errors
///
/// Returns an error if Prometheus rejects a histogram descriptor.
fn register_chain_histograms(recorder: &mut Recorder) -> Result<ChainMetrics> {
    let buckets = LATENCY_BUCKETS_SECONDS.to_vec();
    let fork_choice_block_processing = recorder.histogram(
        "lean_fork_choice_block_processing_time_seconds",
        "Wall time of the fork-choice head recompute (accept_new_votes) on the \
         block-import path. Trigger for the incremental fork-choice and \
         prune-below-finalized levers.",
        buckets.clone(),
    )?;
    let state_transition = recorder.histogram(
        "lean_state_transition_time_seconds",
        "Wall time of a full state transition per imported block, measured at the \
         runtime boundary. Trigger for the per-block state-clone lever.",
        buckets,
    )?;
    Ok(ChainMetrics::new(
        fork_choice_block_processing,
        state_transition,
    ))
}

/// Builds the selected `Store` backend as an `Arc<dyn Store>`.
fn build_store(kind: &StorageKind) -> Result<Arc<dyn Store>> {
    match kind {
        StorageKind::Memory => Ok(Arc::new(MemoryStore::default())),
        StorageKind::Persistent(path) => {
            let store = RedbStore::new(path).context("open persistent store")?;
            Ok(Arc::new(store))
        }
    }
}

/// Loads the persisted head anchor `(head_root, state, block)` when a prior run
/// left one on disk, so a restarted node resumes from its own head instead of
/// genesis. `head_root` is the recorded head root the payload was keyed under;
/// the caller checks the re-anchored engine resolves to it.
///
/// Returns `Ok(None)` for a fresh store, or when a head is recorded but its
/// block/state payload is absent (an inconsistent on-disk view — fall back to a
/// clean genesis anchor rather than resuming from it).
fn resume_anchor(store: &dyn Store) -> Result<Option<(Bytes32, State, Block)>> {
    let Some(head) = store.load_head().context("load persisted head")? else {
        return Ok(None);
    };
    let head_root = head.head.root;
    let (Some(block), Some(state)) = (
        store
            .load_block(&head_root)
            .context("load persisted head block")?,
        store
            .load_state(&head_root)
            .context("load persisted head state")?,
    ) else {
        return Ok(None);
    };
    Ok(Some((head_root, state, block.message)))
}

fn persist_anchor(
    store: &dyn Store,
    anchor_root: Bytes32,
    anchor_slot: Slot,
    finalized: Checkpoint,
    signed_anchor: SignedBlock,
    anchor_state: State,
) -> Result<()> {
    store
        .save_block(anchor_root, signed_anchor)
        .context("persist genesis anchor block")?;
    store
        .save_state(anchor_root, anchor_state)
        .context("persist genesis anchor state")?;
    // Validate the anchor head at the deser seam: genesis (finalized == head
    // at slot 0) is accepted; a finalized checkpoint ahead of the head is
    // refused before it reaches storage.
    let head = HeadInfo::try_new(Checkpoint::new(anchor_root, anchor_slot), finalized)
        .context("validate genesis anchor head")?;
    store
        .save_head(head)
        .context("persist genesis anchor head")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[expect(deprecated)]
mod tests {
    use super::*;
    use runtime::duties::GenesisTimeUnix;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const VALIDATORS_PATH: &str = "../runtime/tests/duties_fixtures/validators.yaml";

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn future_genesis() -> GenesisTimeUnix {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        GenesisTimeUnix::new(now + 60)
    }

    fn validators_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(VALIDATORS_PATH)
    }

    fn build_config(identity_dir: &Path) -> Config {
        let p2p = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &identity_dir.join("identity.pb"),
            None,
        )
        .unwrap();
        let duties = runtime::duties::Config::default()
            .with_validators_path(validators_path())
            .unwrap()
            .with_genesis_time_unix(future_genesis());
        let (genesis_state, genesis_block) = runtime::chain::engine::test_fixtures::anchor_pair(4);

        Config {
            node: NodeConfig::default(),
            p2p,
            duties,
            http_addr: loopback(),
            metrics_addr: loopback(),
            genesis_state,
            genesis_block,
            storage: StorageKind::Memory,
        }
    }

    const SINGLE_NODE_VALIDATORS: &str = "tests/fixtures/single_node_validators.yaml";

    fn single_node_validators_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(SINGLE_NODE_VALIDATORS)
    }

    fn past_genesis() -> GenesisTimeUnix {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        // Non-zero (passes `ensure_runnable`) but in the past, so the driver
        // anchors at `Instant::now()` and slot 0 starts immediately.
        GenesisTimeUnix::new(now.saturating_sub(5))
    }

    /// Single-process end-to-end: one node owning all four engine validators
    /// self-drives — proposes at each slot boundary, attests at vote-due, and
    /// advances the forkchoice clock — with no second node/process. Uses
    /// `start_paused` + `advance` to fire the driver's interval ticker
    /// deterministically.
    #[tokio::test(start_paused = true, flavor = "current_thread")]
    async fn self_driving_node_proposes_attests_and_advances() {
        use crate::consensus_loop::ConsensusLoop;
        use runtime::core::Service as _;
        use tokio_util::sync::CancellationToken;

        let identity_dir = tempfile::tempdir().unwrap();
        let duties = runtime::duties::Config::default()
            .with_validators_path(single_node_validators_path())
            .unwrap()
            .with_validator_group("solo")
            .unwrap()
            .with_genesis_time_unix(past_genesis());
        let p2p_options = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &identity_dir.path().join("id"),
            None,
        )
        .unwrap();

        // Wire the same graph as `new_devnet`, keeping the chain handle so the
        // test can observe head / clock / finalization.
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        let (genesis_state, genesis_block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
        let anchor_slot = genesis_block.slot;
        let signed_anchor = SignedBlock {
            message: genesis_block.clone(),
            signature: Bytes4000::default(),
        };
        let engine =
            runtime::chain::engine::Engine::from_anchor(genesis_state.clone(), genesis_block)
                .unwrap();
        let anchor_root = engine.head();
        let finalized = engine.latest_finalized();
        persist_anchor(
            store.as_ref(),
            anchor_root,
            anchor_slot,
            finalized,
            signed_anchor,
            genesis_state,
        )
        .unwrap();
        let chain = Arc::new(ChainService::new(engine, Arc::clone(&store)));
        let rpc_provider = Arc::new(RpcProvider::chain(Arc::clone(&chain), Arc::clone(&store)));
        let p2p = Arc::new(DevnetHost::build_with_provider(p2p_options, rpc_provider).unwrap());
        let sync = Arc::new(SyncLoop::new(
            SyncConfig::default(),
            Arc::clone(&chain),
            Arc::clone(&p2p),
        ));
        let driver = ConsensusLoop::new(
            Arc::clone(&chain),
            Arc::clone(&p2p),
            Arc::clone(&sync),
            &duties,
        )
        .unwrap();

        p2p.start().await.unwrap();
        driver.start().await.unwrap();

        // Advance enough intervals to cross several slot boundaries and a
        // finalization window. Each `advance` fires exactly one ticker tick;
        // the driver processes that tick's drain/propose/attest/advance
        // sequentially within one handler, so a single `yield_now` (one
        // cooperative hand-off on this current-thread runtime) is sufficient
        // for the task to fully process the tick before the next `advance`.
        // The assertions are threshold-based, leaving slack if that changes.
        for _ in 0..(6 * config::INTERVALS_PER_SLOT + 2) {
            tokio::time::advance(Duration::from_secs(config::SECONDS_PER_INTERVAL)).await;
            tokio::task::yield_now().await;
        }

        let snap = chain.snapshot();
        assert!(
            snap.current_slot >= 3,
            "forkchoice clock must advance >= 3 slots, got {}",
            snap.current_slot,
        );
        assert_ne!(
            snap.head_root, anchor_root,
            "head must move off the genesis anchor (blocks proposed and imported)",
        );
        assert!(
            snap.latest_justified.slot.get() > 0,
            "a checkpoint must justify (votes counted), got justified slot {}",
            snap.latest_justified.slot.get(),
        );
        assert!(
            snap.latest_finalized.slot.get() > 0,
            "a checkpoint must finalize, got finalized slot {}",
            snap.latest_finalized.slot.get(),
        );

        driver.stop(CancellationToken::new()).await.unwrap();
        p2p.stop(CancellationToken::new()).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_devnet_builds_node_that_starts_and_stops() {
        assert!(validators_path().exists());
        let identity_dir = tempfile::tempdir().unwrap();
        let node = new_devnet(build_config(identity_dir.path())).unwrap();

        node.start().await.unwrap();
        node.status().await.unwrap();
        node.stop().await.unwrap();
    }

    #[test]
    fn register_chain_gauges_freezes_without_collision() {
        // The wired chain gauges must not collide with each other or the
        // baseline gauges, so `freeze` succeeds.
        let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
        let engine = runtime::chain::engine::Engine::from_anchor(state, block).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        let chain = Arc::new(ChainService::new(engine, store));

        let mut recorder = Recorder::new();
        register_chain_gauges(&mut recorder, &chain);
        assert!(recorder.freeze().is_ok());
    }

    #[test]
    fn register_chain_histograms_wires_two_triggers_only() {
        // The composition-root registration wires exactly the two boundary-
        // observable trigger histograms and omits the deferred process-slots one.
        let mut recorder = Recorder::new();
        let _metrics = register_chain_histograms(&mut recorder).unwrap();

        let body = recorder.freeze().unwrap().encode().unwrap();
        assert!(body.contains("lean_fork_choice_block_processing_time_seconds"));
        assert!(body.contains("lean_state_transition_time_seconds"));
        assert!(!body.contains("lean_state_transition_slots_processing_time_seconds"));
    }

    #[test]
    fn persist_anchor_seeds_head_block_and_state() {
        let store = MemoryStore::default();
        let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
        let slot = block.slot;
        let engine =
            runtime::chain::engine::Engine::from_anchor(state.clone(), block.clone()).unwrap();
        let root = engine.head();
        let finalized = engine.latest_finalized();
        let signed = SignedBlock {
            message: block,
            signature: Bytes4000::default(),
        };

        persist_anchor(&store, root, slot, finalized, signed.clone(), state).unwrap();

        assert_eq!(store.load_block(&root).unwrap(), Some(signed));
        assert!(store.load_state(&root).unwrap().is_some());
        assert_eq!(
            store.load_head().unwrap(),
            Some(HeadInfo::new(Checkpoint::new(root, slot), finalized))
        );
    }

    // Test A: the production `resume_anchor` helper across all three branches.
    // `State`/`Block` compare via their derived `PartialEq`, so no hash-tree-root
    // import is needed to assert the resumed anchor matches what was persisted.
    #[test]
    fn resume_anchor_covers_empty_seeded_and_inconsistent() {
        // (1) Fresh store → no head → None (genesis path).
        let empty = MemoryStore::default();
        assert!(resume_anchor(&empty).unwrap().is_none());

        // Build a valid anchor and persist it head-consistently.
        let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
        let engine =
            runtime::chain::engine::Engine::from_anchor(state.clone(), block.clone()).unwrap();
        let root = engine.head();
        let slot = block.slot;
        let finalized = engine.latest_finalized();
        let signed = SignedBlock {
            message: block.clone(),
            signature: Bytes4000::default(),
        };

        // (2) Head + block + state present → Some((root, state, block)) at head.
        let full = MemoryStore::default();
        persist_anchor(&full, root, slot, finalized, signed, state.clone()).unwrap();
        let (got_root, got_state, got_block) =
            resume_anchor(&full).unwrap().expect("resume from head");
        assert_eq!(got_root, root, "resume returns the recorded head root");
        assert_eq!(
            got_block, block,
            "resumed block matches persisted head block"
        );
        assert_eq!(
            got_state, state,
            "resumed state matches persisted head state"
        );

        // (3) Head recorded but payload absent → None (inconsistent-view fallback).
        let head_only = MemoryStore::default();
        head_only
            .save_head(HeadInfo::new(Checkpoint::new(root, slot), finalized))
            .unwrap();
        assert!(
            resume_anchor(&head_only).unwrap().is_none(),
            "head without block/state payload must fall back to a clean anchor"
        );
    }

    // Test B: the production `new_devnet` resume arm. Seed a persistent store,
    // drop it, then construct a node pointed at the same path and assert it
    // builds/starts/stops via the `Some(resume)` branch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_devnet_resumes_from_persistent_store() {
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("chain.redb");

        // Seed the store with a head-consistent genesis anchor, then drop it so
        // redb releases the file lock before new_devnet reopens the same path.
        {
            let store = RedbStore::new(&db_path).unwrap();
            let (state, block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
            let engine =
                runtime::chain::engine::Engine::from_anchor(state.clone(), block.clone()).unwrap();
            let root = engine.head();
            let signed = SignedBlock {
                message: block.clone(),
                signature: Bytes4000::default(),
            };
            persist_anchor(
                &store,
                root,
                block.slot,
                engine.latest_finalized(),
                signed,
                state,
            )
            .unwrap();
        }

        let identity_dir = tempfile::tempdir().unwrap();
        let mut config = build_config(identity_dir.path());
        config.storage = StorageKind::Persistent(db_path);
        // Resume branch executes here; node must build, start, and stop cleanly.
        let node = new_devnet(config).unwrap();
        node.start().await.unwrap();
        node.stop().await.unwrap();
    }

    // Drives a fresh persistent node (RedbStore at `db_path`) to finalization,
    // then drops every store holder so redb releases the on-disk file lock.
    // Returns the finalized `(head_root, finalized_slot)`. Mirrors the manual
    // graph of `self_driving_node_proposes_attests_and_advances`.
    async fn drive_persistent_node_to_finalization(
        db_path: &Path,
        identity_dir: &Path,
    ) -> (Bytes32, u64) {
        use crate::consensus_loop::ConsensusLoop;
        use runtime::core::Service as _;
        use tokio_util::sync::CancellationToken;

        let duties = runtime::duties::Config::default()
            .with_validators_path(single_node_validators_path())
            .unwrap()
            .with_validator_group("solo")
            .unwrap()
            .with_genesis_time_unix(past_genesis());
        let p2p_options = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &identity_dir.join("id"),
            None,
        )
        .unwrap();

        let store: Arc<dyn Store> = Arc::new(RedbStore::new(db_path).unwrap());
        let (genesis_state, genesis_block) = runtime::chain::engine::test_fixtures::anchor_pair(4);
        let anchor_slot = genesis_block.slot;
        let signed_anchor = SignedBlock {
            message: genesis_block.clone(),
            signature: Bytes4000::default(),
        };
        let engine =
            runtime::chain::engine::Engine::from_anchor(genesis_state.clone(), genesis_block)
                .unwrap();
        let anchor_root = engine.head();
        let finalized = engine.latest_finalized();
        persist_anchor(
            store.as_ref(),
            anchor_root,
            anchor_slot,
            finalized,
            signed_anchor,
            genesis_state,
        )
        .unwrap();
        let chain = Arc::new(ChainService::new(engine, Arc::clone(&store)));
        let rpc_provider = Arc::new(RpcProvider::chain(Arc::clone(&chain), Arc::clone(&store)));
        let p2p = Arc::new(DevnetHost::build_with_provider(p2p_options, rpc_provider).unwrap());
        let sync = Arc::new(SyncLoop::new(
            SyncConfig::default(),
            Arc::clone(&chain),
            Arc::clone(&p2p),
        ));
        let driver = ConsensusLoop::new(
            Arc::clone(&chain),
            Arc::clone(&p2p),
            Arc::clone(&sync),
            &duties,
        )
        .unwrap();

        p2p.start().await.unwrap();
        driver.start().await.unwrap();
        for _ in 0..(6 * config::INTERVALS_PER_SLOT + 2) {
            tokio::time::advance(Duration::from_secs(config::SECONDS_PER_INTERVAL)).await;
            tokio::task::yield_now().await;
        }
        let snap = chain.snapshot();
        assert!(
            snap.latest_finalized.slot.get() > 0,
            "run 1 must finalize before restart, got finalized slot {}",
            snap.latest_finalized.slot.get(),
        );
        driver.stop(CancellationToken::new()).await.unwrap();
        p2p.stop(CancellationToken::new()).await.unwrap();
        let result = (snap.head_root, snap.latest_finalized.slot.get());
        // Drop every Arc<dyn Store> holder so redb releases the file lock before
        // the caller reopens the same path.
        drop((store, chain, sync, driver, p2p));
        result
    }

    // Test C: end-to-end restart durability. Drive a persistent node to
    // finalization, drop it, reopen the same path, resume via
    // `Engine::from_trusted_head`, and drive the forkchoice head recompute
    // (`tick_interval`) over a full slot — the exact operation that errored with
    // `UnknownRootBlock` before the fix. The contrast against a `from_anchor`-built
    // engine on the SAME persisted data proves the fix end-to-end. (We drive
    // `tick_interval` directly rather than a fresh `ConsensusLoop`, whose tick
    // counter starts at slot 0 and so cannot extend a resumed higher-slot head —
    // a separate concern from the forkchoice resume bug this fixes.)
    #[tokio::test(start_paused = true, flavor = "current_thread")]
    async fn persistent_node_restart_resumes_forkchoice_head_recompute() {
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("chain.redb");
        let identity_dir = tempfile::tempdir().unwrap();

        // ----- Run 1: drive a persistent node to finalization, then drop it ---
        let (head_root, finalized_slot) =
            drive_persistent_node_to_finalization(&db_path, identity_dir.path()).await;

        // ----- Run 2: reopen, resume via from_trusted_head, drive the head
        //       recompute the bug used to break -----
        // First confirm the raw head/finalized records reload verbatim.
        let store: Arc<dyn Store> = Arc::new(RedbStore::new(&db_path).unwrap());
        let head = store.load_head().unwrap().expect("head reloads from disk");
        assert_eq!(head.head.root, head_root, "head root reloads from disk");
        assert_eq!(
            head.finalized.slot.get(),
            finalized_slot,
            "finalized slot reloads from disk"
        );

        let (resumed_root, state, block) = resume_anchor(store.as_ref())
            .unwrap()
            .expect("resume_anchor recovers the persisted head anchor");
        assert_eq!(
            resumed_root, head_root,
            "resume returns the recorded head root"
        );

        // The persisted head finalized past genesis, so its justified checkpoint
        // is a real ancestor block (non-zero root) absent from a single-anchor
        // map — exactly the condition that broke the old from_anchor resume.
        assert_ne!(state.latest_justified.root, head_root);
        assert_ne!(state.latest_justified.root, Bytes32::zero());

        // OLD path (from_anchor): advancing the forkchoice clock over one slot of
        // intervals reaches a head-walk phase, where get_fork_choice_head hits
        // the absent justified root and errors.
        let broken =
            runtime::chain::engine::Engine::from_anchor(state.clone(), block.clone()).unwrap();
        let broken_result =
            (0..config::INTERVALS_PER_SLOT).try_for_each(|_| broken.tick_interval(false));
        assert!(
            broken_result.is_err(),
            "from_anchor resume must fail the head recompute (UnknownRootBlock)",
        );

        // FIX (from_trusted_head): resumes at the persisted head and the head
        // recompute succeeds across a full slot of intervals — the node keeps
        // running instead of erroring on its first consensus tick.
        let resumed = runtime::chain::engine::Engine::from_trusted_head(state, block).unwrap();
        assert_eq!(
            resumed.head(),
            head_root,
            "resumed engine anchors at the persisted head",
        );
        for _ in 0..config::INTERVALS_PER_SLOT {
            resumed
                .tick_interval(false)
                .expect("resumed head recompute must succeed after restart");
        }
        assert!(
            resumed.latest_finalized().slot.get() >= finalized_slot,
            "finalized stays monotonic across restart",
        );
    }
}
