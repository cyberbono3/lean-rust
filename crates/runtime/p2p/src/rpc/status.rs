//! `Status` handshake handler.
//!
//! Both peers send a Status request on `ConnectionEstablished`. Each
//! side validates the peer's Status against its own ([`validate`]) and
//! either responds (inbound path) or accepts the response (outbound
//! path); on mismatch the peer is disconnected.

use lean_wire::Status;
use libp2p::{request_response::ResponseChannel, PeerId, Swarm};
use tracing::{info, warn};

use super::{RpcProvider, RpcResponse};
use crate::host::behaviour::DevnetBehaviour;

/// Rejects obviously-different-fork peers; accepts peer-ahead or
/// peer-behind on the same fork.
///
/// Devnet0-permissive predicate: same finalized slot ⇒ roots must
/// agree, otherwise one party is ahead and they can sync from each
/// other regardless. Mainnet would tighten this (chain-id /
/// fork-digest); confirm against spec at review time.
#[must_use]
pub(crate) fn validate(local: &Status, peer: &Status) -> bool {
    local.finalized.slot != peer.finalized.slot || local.finalized.root == peer.finalized.root
}

/// Inbound `Status` request: respond with the local Status if the peer
/// validates, otherwise disconnect.
pub(crate) fn on_inbound(
    peer: PeerId,
    peer_status: &Status,
    channel: ResponseChannel<RpcResponse>,
    swarm: &mut Swarm<DevnetBehaviour>,
    provider: &dyn RpcProvider,
) {
    let local = provider.local_status();
    if !validate(&local, peer_status) {
        disconnect_on_mismatch(peer, &local, peer_status, swarm, "inbound request");
        return;
    }
    let _ = swarm
        .behaviour_mut()
        .status_rr
        .send_response(channel, RpcResponse::Status(local));
    log_handshake_ok(peer, &local, peer_status, "inbound");
}

/// Outbound `Status` response: validate the peer's Status and
/// disconnect on mismatch. The response value is otherwise discarded —
/// we have nothing to forward.
pub(crate) fn on_outbound_response(
    peer: PeerId,
    peer_status: &Status,
    swarm: &mut Swarm<DevnetBehaviour>,
    provider: &dyn RpcProvider,
) {
    let local = provider.local_status();
    if !validate(&local, peer_status) {
        disconnect_on_mismatch(peer, &local, peer_status, swarm, "outbound response");
        return;
    }
    log_handshake_ok(peer, &local, peer_status, "outbound");
}

fn log_handshake_ok(peer: PeerId, local: &Status, peer_status: &Status, direction: &'static str) {
    info!(
        peer = %peer,
        direction,
        local_finalized_slot = local.finalized.slot.get(),
        local_finalized_root = %local.finalized.root.to_hex(),
        local_head_slot = local.head.slot.get(),
        local_head_root = %local.head.root.to_hex(),
        peer_finalized_slot = peer_status.finalized.slot.get(),
        peer_finalized_root = %peer_status.finalized.root.to_hex(),
        peer_head_slot = peer_status.head.slot.get(),
        peer_head_root = %peer_status.head.root.to_hex(),
        "status handshake accepted",
    );
}

/// Logs the mismatch and tears down the peer connection. `direction`
/// (`"inbound request"` or `"outbound response"`) names the half of
/// the handshake that failed for forensic logs.
fn disconnect_on_mismatch(
    peer: PeerId,
    local: &Status,
    peer_status: &Status,
    swarm: &mut Swarm<DevnetBehaviour>,
    direction: &'static str,
) {
    warn!(
        peer = %peer,
        direction,
        ?local,
        peer_status = ?peer_status,
        "status handshake rejected; disconnecting",
    );
    let _ = swarm.disconnect_peer_id(peer);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::{Checkpoint, Slot};
    use types::Bytes32;

    fn status(finalized_slot: u64, finalized_root: u8, head_slot: u64, head_root: u8) -> Status {
        Status {
            finalized: Checkpoint::new(
                Bytes32::new([finalized_root; 32]),
                Slot::new(finalized_slot),
            ),
            head: Checkpoint::new(Bytes32::new([head_root; 32]), Slot::new(head_slot)),
        }
    }

    #[test]
    fn validate_table() {
        // (case, local, peer, expected)
        // Same finalized slot ⇒ roots must agree; different slot ⇒ accept.
        // The `default` case is load-bearing: two NoOpRpcProvider peers
        // both report Status::default(); lifecycle tests rely on the
        // handshake succeeding so the connection is not torn down.
        let cases = [
            (
                "identical",
                status(10, 0xAA, 20, 0xBB),
                status(10, 0xAA, 20, 0xBB),
                true,
            ),
            ("default", Status::default(), Status::default(), true),
            (
                "same_slot_different_root",
                status(10, 0xAA, 20, 0xCC),
                status(10, 0xBB, 20, 0xDD),
                false,
            ),
            (
                "peer_ahead",
                status(10, 0xAA, 20, 0xCC),
                status(15, 0xBB, 25, 0xDD),
                true,
            ),
            (
                "peer_behind",
                status(20, 0xAA, 30, 0xCC),
                status(10, 0xBB, 15, 0xDD),
                true,
            ),
        ];
        for (case, local, peer, expected) in cases {
            assert_eq!(validate(&local, &peer), expected, "case {case}");
        }
    }
}
