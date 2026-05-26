//! Adapter that lets p2p answer RPCs from chain and storage.

use std::sync::Arc;

use lean_chain::Service as ChainService;
use networking::Status;
use protocol::SignedBlock;
use runtime_p2p::RpcProvider;
use storage::Store;
use tracing::warn;
use types::Bytes32;

pub(crate) struct RpcProviderAdapter {
    chain: Arc<ChainService>,
    store: Arc<dyn Store>,
}

impl RpcProviderAdapter {
    pub(crate) fn new(chain: Arc<ChainService>, store: Arc<dyn Store>) -> Self {
        Self { chain, store }
    }
}

impl RpcProvider for RpcProviderAdapter {
    fn get_block_by_root(&self, root: &Bytes32) -> Option<SignedBlock> {
        match self.store.load_block(root) {
            Ok(block) => block,
            Err(err) => {
                warn!(?root, %err, "p2p rpc block lookup failed");
                None
            }
        }
    }

    fn local_status(&self) -> Status {
        self.chain.local_status()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::{Checkpoint, Slot};
    use storage::{HeadInfo, MemoryStore};

    fn build_adapter() -> (RpcProviderAdapter, Arc<dyn Store>) {
        let (state, block) = engine::test_fixtures::anchor_pair(4);
        let engine = engine::Engine::from_anchor(state, block).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        let chain = Arc::new(ChainService::new(engine, Arc::clone(&store)));
        (RpcProviderAdapter::new(chain, Arc::clone(&store)), store)
    }

    #[test]
    fn local_status_comes_from_chain_snapshot() {
        let (adapter, _store) = build_adapter();
        let status = adapter.local_status();

        assert_eq!(status.head.slot, Slot::ZERO);
        assert_eq!(
            status.finalized,
            Checkpoint::new(status.head.root, Slot::ZERO)
        );
    }

    #[test]
    fn get_block_by_root_reads_storage() {
        let (adapter, store) = build_adapter();
        let root = Bytes32::new([0x42; 32]);
        let block = SignedBlock::default();

        store.save_block(root, block.clone()).unwrap();

        assert_eq!(adapter.get_block_by_root(&root), Some(block));
        assert_eq!(adapter.get_block_by_root(&Bytes32::new([0x24; 32])), None);
    }

    #[test]
    fn get_block_by_root_returns_none_for_missing_block() {
        let (adapter, _store) = build_adapter();

        assert!(adapter
            .get_block_by_root(&Bytes32::new([0x11; 32]))
            .is_none());
    }

    #[test]
    fn storage_head_does_not_drive_local_status() {
        let (adapter, store) = build_adapter();
        store
            .save_head(HeadInfo::new(
                Checkpoint::new(Bytes32::new([0xAA; 32]), Slot::new(9)),
                Checkpoint::new(Bytes32::new([0xBB; 32]), Slot::new(3)),
            ))
            .unwrap();

        assert_eq!(adapter.local_status().head.slot, Slot::ZERO);
    }
}
