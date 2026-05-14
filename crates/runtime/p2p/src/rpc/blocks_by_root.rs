//! Inbound `BlocksByRoot` handler.
//!
//! Looks up each requested root via the injected [`RpcProvider`],
//! builds a [`BlocksByRootResponse`] from the present blocks, and sends
//! it back over the [`ResponseChannel`]. Unknown roots are silently
//! dropped — the resulting response is shorter than the request length
//! when some roots are missing, and empty when all are.

use libp2p::{request_response::ResponseChannel, PeerId, Swarm};
use networking::{BlocksByRootRequest, BlocksByRootResponse};
use tracing::{debug, warn};

use super::{RpcProvider, RpcResponse};
use crate::host::behaviour::DevnetBehaviour;

/// Pure helper: looks up each requested root via `provider` and folds
/// the present blocks into a [`BlocksByRootResponse`]. Unknown roots
/// are dropped silently — the response is empty when every root is
/// missing.
///
/// The `BlocksByRootResponse::new` constructor enforces the
/// [`networking::MAX_REQUEST_BLOCKS`] cap. The request side is
/// independently capped at SSZ-decode time, so the cap is never
/// exceeded here in practice; on the unreachable error path we surface
/// a warning and return an empty response.
pub(crate) fn build_response(
    request: &BlocksByRootRequest,
    provider: &dyn RpcProvider,
) -> BlocksByRootResponse {
    let blocks: Vec<_> = request
        .roots()
        .iter()
        .filter_map(|root| provider.get_block_by_root(root))
        .collect();
    BlocksByRootResponse::new(blocks).unwrap_or_else(|err| {
        warn!(%err, "blocks_by_root response build failed; sending empty");
        BlocksByRootResponse::default()
    })
}

/// Inbound `BlocksByRoot` request handler — composes
/// [`build_response`] with the swarm-side `send_response` call.
pub(crate) fn on_inbound(
    peer: PeerId,
    request: &BlocksByRootRequest,
    channel: ResponseChannel<RpcResponse>,
    swarm: &mut Swarm<DevnetBehaviour>,
    provider: &dyn RpcProvider,
) {
    let response = build_response(request, provider);
    debug!(
        peer = %peer,
        requested = request.roots().len(),
        returned = response.blocks().len(),
        "blocks_by_root respond",
    );
    let _ = swarm
        .behaviour_mut()
        .request_response
        .send_response(channel, RpcResponse::BlocksByRoot(response));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use networking::Status;
    use protocol::SignedBlock;
    use std::collections::HashMap;
    use types::Bytes32;

    /// Stub provider that answers from an in-memory map. Unknown roots
    /// return `None`.
    struct MapProvider {
        blocks: HashMap<Bytes32, SignedBlock>,
    }

    impl RpcProvider for MapProvider {
        fn get_block_by_root(&self, root: &Bytes32) -> Option<SignedBlock> {
            self.blocks.get(root).cloned()
        }

        fn local_status(&self) -> Status {
            Status::default()
        }
    }

    fn root(byte: u8) -> Bytes32 {
        Bytes32::new([byte; 32])
    }

    #[test]
    fn returns_known_blocks() {
        let mut blocks = HashMap::new();
        blocks.insert(root(0x11), SignedBlock::default());
        blocks.insert(root(0x22), SignedBlock::default());
        let provider = MapProvider { blocks };

        let request = BlocksByRootRequest::new(vec![root(0x11), root(0x22)]).unwrap();
        let response = build_response(&request, &provider);

        assert_eq!(response.blocks().len(), 2);
    }

    #[test]
    fn unknown_roots_yield_empty_response() {
        let provider = MapProvider {
            blocks: HashMap::new(),
        };
        let request = BlocksByRootRequest::new(vec![root(0x11), root(0x22), root(0x33)]).unwrap();
        let response = build_response(&request, &provider);

        assert!(response.blocks().is_empty());
    }

    #[test]
    fn mixed_known_and_unknown_returns_only_known() {
        let mut blocks = HashMap::new();
        blocks.insert(root(0x11), SignedBlock::default());
        let provider = MapProvider { blocks };

        let request = BlocksByRootRequest::new(vec![root(0x11), root(0xFF)]).unwrap();
        let response = build_response(&request, &provider);

        assert_eq!(response.blocks().len(), 1);
    }
}
