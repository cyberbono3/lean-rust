//! `Status` handshake handler.
//!
//! Both peers send a Status request on `ConnectionEstablished`. Each
//! side validates the peer's Status against its own ([`validate`]) and
//! either responds (inbound path) or accepts the response (outbound
//! path); on mismatch the peer is disconnected.

use libp2p::{request_response::ResponseChannel, PeerId, Swarm};
use networking::Status;
use tracing::{debug, warn};

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
    use std::cmp::Ordering;
    match local.finalized.slot.cmp(&peer.finalized.slot) {
        Ordering::Equal => local.finalized.root == peer.finalized.root,
        Ordering::Less | Ordering::Greater => true,
    }
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
    if validate(&local, peer_status) {
        let _ = swarm
            .behaviour_mut()
            .request_response
            .send_response(channel, RpcResponse::Status(local));
        debug!(peer = %peer, "status handshake ok (inbound)");
    } else {
        warn!(
            peer = %peer,
            ?local,
            peer_status = ?peer_status,
            "status mismatch on inbound request; disconnecting",
        );
        let _ = swarm.disconnect_peer_id(peer);
    }
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
    if validate(&local, peer_status) {
        debug!(peer = %peer, "status handshake ok (outbound)");
    } else {
        warn!(
            peer = %peer,
            ?local,
            peer_status = ?peer_status,
            "status mismatch on outbound response; disconnecting",
        );
        let _ = swarm.disconnect_peer_id(peer);
    }
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
    fn validate_accepts_identical_status() {
        let s = status(10, 0xAA, 20, 0xBB);
        assert!(validate(&s, &s));
    }

    #[test]
    fn validate_accepts_default() {
        // Two NoOpRpcProvider peers both report Status::default();
        // handshake must succeed so lifecycle tests don't disconnect.
        assert!(validate(&Status::default(), &Status::default()));
    }

    #[test]
    fn validate_rejects_same_slot_different_root() {
        let local = status(10, 0xAA, 20, 0xCC);
        let peer = status(10, 0xBB, 20, 0xDD);
        assert!(!validate(&local, &peer));
    }

    #[test]
    fn validate_accepts_peer_ahead() {
        let local = status(10, 0xAA, 20, 0xCC);
        let peer = status(15, 0xBB, 25, 0xDD);
        assert!(validate(&local, &peer));
    }

    #[test]
    fn validate_accepts_peer_behind() {
        let local = status(20, 0xAA, 30, 0xCC);
        let peer = status(10, 0xBB, 15, 0xDD);
        assert!(validate(&local, &peer));
    }
}
