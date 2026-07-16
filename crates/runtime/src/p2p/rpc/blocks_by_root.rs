//! Inbound `BlocksByRoot` handler.
//!
//! Looks up each requested root via the injected [`RpcProvider`],
//! builds a [`BlocksByRootResponse`] from the present blocks, and sends
//! it back over the [`ResponseChannel`]. Unknown roots are silently
//! dropped — the resulting response is shorter than the request length
//! when some roots are missing, and empty when all are.

use lean_wire::{BlocksByRootRequest, BlocksByRootResponse};
use libp2p::{request_response::ResponseChannel, PeerId, Swarm};
use tracing::{debug, warn};

use super::{RpcProvider, RpcResponse};
use crate::p2p::host::behaviour::DevnetBehaviour;

/// Pure helper backing [`on_inbound`]. See the module-level doc for the
/// drop-unknown-roots contract; this function holds no side effects
/// beyond the provider lookups.
pub(crate) fn build_response(
    request: &BlocksByRootRequest,
    provider: &RpcProvider,
) -> BlocksByRootResponse {
    let blocks = request
        .roots()
        .iter()
        .filter_map(|root| provider.get_block_by_root(root));
    // The `BlocksByRootResponse::new` constructor enforces the
    // `lean_wire::MAX_REQUEST_BLOCKS` cap. The request side is
    // independently capped at SSZ-decode time and `filter_map` only
    // ever shrinks the iterator, so the cap is never exceeded here in
    // practice; the warn-and-default branch is defensive belt-and-
    // suspenders for a future invariant change.
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
    provider: &RpcProvider,
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
        .blocks_rr
        .send_response(channel, RpcResponse::BlocksByRoot(response));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::SignedBlockWithAttestation;
    use std::sync::Arc;
    use storage::{MemoryStore, Store};
    use types::Bytes32;

    fn root(byte: u8) -> Bytes32 {
        Bytes32::new([byte; 32])
    }

    /// Builds a `Chain` provider whose block store is seeded with the
    /// given roots (each mapped to a default block). Unknown roots return
    /// `None`. The chain handle is a genesis fixture; only the store is
    /// exercised by `get_block_by_root`.
    fn provider_with(known: &[u8]) -> RpcProvider {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        for &b in known {
            store
                .save_block(root(b), SignedBlockWithAttestation::default())
                .unwrap();
        }
        let (state, block) = crate::chain::engine::test_fixtures::anchor_pair(4);
        let engine = crate::chain::engine::Engine::from_anchor(state, block).unwrap();
        let chain = Arc::new(crate::chain::Service::new(engine, Arc::clone(&store)));
        RpcProvider::chain(chain, store)
    }

    #[test]
    fn build_response_returns_only_known_roots() {
        // (case, known_roots, requested_roots, expected_block_count)
        let cases: &[(&str, &[u8], &[u8], usize)] = &[
            ("all_known", &[0x11, 0x22], &[0x11, 0x22], 2),
            ("none_known", &[], &[0x11, 0x22, 0x33], 0),
            ("partial_overlap", &[0x11], &[0x11, 0xFF], 1),
        ];
        for &(case, known, requested, expected) in cases {
            let provider = provider_with(known);
            let request = BlocksByRootRequest::new(requested.iter().copied().map(root)).unwrap();
            let response = build_response(&request, &provider);
            assert_eq!(response.blocks().len(), expected, "case {case}");
        }
    }
}
