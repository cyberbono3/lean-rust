//! Devnet composition entry point.

use std::net::SocketAddr;
use std::sync::Arc;

use protocol::{Block, State};
use runtime_api::{HttpService, MetricsService, Recorder};
use runtime_chain::Service as ChainService;
use runtime_core::{Node, NodeConfig};
use runtime_p2p::{DevnetHost, HostOptions, RpcProvider};
use storage::{MemoryStore, Store};

use crate::gossip_ingest::GossipIngestService;
use crate::publisher_adapter::PublisherAdapter;
use crate::rpc_provider::RpcProviderAdapter;

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
    pub duties: runtime_duties::Config,
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
/// status-request hooks required by `runtime-sync`, so peer backfill sync
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
    let engine = engine::Engine::from_anchor(genesis_state, genesis_block)?;
    let chain = Arc::new(ChainService::new(engine, Arc::clone(&store)));

    let rpc_provider: Arc<dyn RpcProvider> = Arc::new(RpcProviderAdapter::new(
        Arc::clone(&chain),
        Arc::clone(&store),
    ));
    let p2p = Arc::new(DevnetHost::build_with_provider(p2p_options, rpc_provider)?);
    let gossip_ingest = Arc::new(GossipIngestService::new(
        Arc::clone(&p2p),
        Arc::clone(&chain),
    ));

    let duties = Arc::new(runtime_duties::Service::new(
        duties,
        chain.clone(),
        Arc::new(PublisherAdapter::new(Arc::clone(&p2p))),
    ));

    let http = Arc::new(HttpService::new(Arc::clone(&store), http_addr));
    let metrics = Arc::new(MetricsService::new(metrics_addr, Recorder::new()));

    Ok(Node::new(node)
        .with_chain(chain)
        .with_p2p(p2p)
        .with_sync(gossip_ingest)
        .with_duties(duties)
        .with_http(http)
        .with_metrics(metrics))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use runtime_duties::GenesisTimeUnix;
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
        let duties = runtime_duties::Config::default()
            .with_validators_path(validators_path())
            .unwrap()
            .with_genesis_time_unix(future_genesis());
        let (genesis_state, genesis_block) = engine::test_fixtures::anchor_pair(4);

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
}
