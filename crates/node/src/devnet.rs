//! Devnet composition entry point.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use lean_api::{HttpService, MetricsService, Recorder};
use lean_chain::Service as ChainService;
use lean_core::{Node, NodeConfig};
use lean_p2p_host::{DevnetHost, HostOptions, RpcProvider};
use protocol::{Block, Checkpoint, SignedBlock, Slot, State};
use storage::{HeadInfo, MemoryStore, Store};
use types::{Bytes32, Bytes4000};

use crate::gossip_ingest::GossipIngestService;

/// Result type returned by node composition.
pub type Result<T> = anyhow::Result<T>;

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
    pub duties: lean_duties::Config,
    /// HTTP API bind address.
    pub http_addr: SocketAddr,
    /// Prometheus metrics bind address.
    pub metrics_addr: SocketAddr,
    /// Trusted genesis state used to anchor the engine.
    pub genesis_state: State,
    /// Trusted genesis block used to anchor the engine.
    pub genesis_block: Block,
}

/// Builds a devnet [`Node`] with concrete runtime services.
///
/// The current p2p surface does not yet expose the clean peer-event and
/// status-request hooks required by `lean-sync`, so peer backfill sync
/// is left unwired here. Gossip ingestion still runs in the sync lifecycle
/// slot so p2p-delivered blocks and votes reach the chain before duties
/// begin producing local messages.
///
/// # Errors
///
/// Returns an error if the engine rejects the genesis anchor or p2p host
/// construction fails.
pub fn new_devnet(config: Config) -> Result<Node> {
    let Config {
        node,
        p2p: p2p_options,
        duties,
        http_addr,
        metrics_addr,
        genesis_state,
        genesis_block,
    } = config;

    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let anchor_slot = genesis_block.slot;
    let anchor_state = genesis_state.clone();
    let signed_anchor = SignedBlock {
        message: genesis_block.clone(),
        signature: Bytes4000::default(),
    };
    let engine = lean_chain::engine::Engine::from_anchor(genesis_state, genesis_block)?;
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
    let chain = Arc::new(ChainService::new(engine, Arc::clone(&store)));

    let rpc_provider = Arc::new(RpcProvider::chain(Arc::clone(&chain), Arc::clone(&store)));
    let p2p = Arc::new(DevnetHost::build_with_provider(p2p_options, rpc_provider)?);
    let gossip_ingest = Arc::new(GossipIngestService::new(
        Arc::clone(&p2p),
        Arc::clone(&chain),
    ));

    let duties = Arc::new(lean_duties::Service::new(
        duties,
        chain.clone(),
        Arc::new(lean_duties::Publisher::new(Arc::clone(&p2p))),
    ));

    let http = Arc::new(HttpService::new(Arc::clone(&store), http_addr));
    let mut recorder = Recorder::new();
    register_chain_gauges(&mut recorder, &chain);
    let metrics = Arc::new(MetricsService::new(metrics_addr, recorder.freeze()?));

    Ok(Node::new(node)
        .with_chain(chain)
        .with_p2p(p2p)
        .with_sync(gossip_ingest)
        .with_duties(duties)
        .with_http(http)
        .with_metrics(metrics))
}

/// Registers chain-state gauges that read the chain service's hot
/// snapshot (`Arc<RwLock<ChainSnapshot>>`). Each closure takes a read
/// lock per scrape — cheap, and decoupled from the engine mutex. Closes
/// the fixture §8 gap where `/metrics` exposed only the two baseline
/// process gauges.
///
/// A connected-peer gauge is intentionally not wired here: the p2p host
/// exposes no synchronous connected-peer count today, so that gauge is
/// deferred to a p2p-touching change that adds the counter.
fn register_chain_gauges(recorder: &mut Recorder, chain: &Arc<ChainService>) {
    let head = chain.snapshot();
    recorder.gauge(
        "lean_chain_slot",
        "Current forkchoice slot (clock).",
        move || head.read().current_slot,
    );

    let justified = chain.snapshot();
    recorder.gauge(
        "lean_chain_justified_slot",
        "Slot of the latest justified checkpoint.",
        move || justified.read().latest_justified.slot.get(),
    );

    let finalized = chain.snapshot();
    recorder.gauge(
        "lean_chain_finalized_slot",
        "Slot of the latest finalized checkpoint.",
        move || finalized.read().latest_finalized.slot.get(),
    );
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
mod tests {
    use super::*;
    use lean_duties::GenesisTimeUnix;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const VALIDATORS_PATH: &str = "../runtime/duties/tests/fixtures/validators.yaml";

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
        let duties = lean_duties::Config::default()
            .with_validators_path(validators_path())
            .unwrap()
            .with_genesis_time_unix(future_genesis());
        let (genesis_state, genesis_block) = lean_chain::engine::test_fixtures::anchor_pair(4);

        Config {
            node: NodeConfig::default(),
            p2p,
            duties,
            http_addr: loopback(),
            metrics_addr: loopback(),
            genesis_state,
            genesis_block,
        }
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
        let (state, block) = lean_chain::engine::test_fixtures::anchor_pair(4);
        let engine = lean_chain::engine::Engine::from_anchor(state, block).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        let chain = Arc::new(ChainService::new(engine, store));

        let mut recorder = Recorder::new();
        register_chain_gauges(&mut recorder, &chain);
        assert!(recorder.freeze().is_ok());
    }

    #[test]
    fn persist_anchor_seeds_head_block_and_state() {
        let store = MemoryStore::default();
        let (state, block) = lean_chain::engine::test_fixtures::anchor_pair(4);
        let slot = block.slot;
        let engine = lean_chain::engine::Engine::from_anchor(state.clone(), block.clone()).unwrap();
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
}
