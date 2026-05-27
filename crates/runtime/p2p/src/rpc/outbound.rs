//! Outbound RPC request correlation.
//!
//! When the swarm task receives a [`crate::host::HostCommand::SendRequest`]
//! command, it calls `request_response::Behaviour::send_request` and
//! gets back an [`OutboundRequestId`]. The reply `oneshot::Sender` is
//! parked in the [`OutboundTable`] keyed by that id, together with the
//! [`PeerId`] it was sent to; when the matching `Event::Message { Response }`
//! or `Event::OutboundFailure` arrives, the table is drained and the caller
//! is woken. On `SwarmEvent::ConnectionClosed` every entry for the departing
//! peer is failed via [`OutboundTable::fail_all_for_peer`] so a dropped
//! connection cannot leak pending entries for the swarm task's lifetime.
//!
//! Lives in the swarm-task's stack frame — single-threaded, no locking.
//!
//! The table is generic over the request-id type, defaulting to libp2p's
//! [`OutboundRequestId`]; tests instantiate it with `u64` keys because
//! `OutboundRequestId` has no public constructor.

use std::collections::HashMap;
use std::hash::Hash;

use libp2p::request_response::OutboundRequestId;
use libp2p::PeerId;
use tokio::sync::oneshot;

use super::{RpcError, RpcResponse};

/// Reply channel parked for one in-flight outbound request.
type Reply = oneshot::Sender<Result<RpcResponse, RpcError>>;

/// Pending outbound requests waiting on a libp2p response or failure
/// event. Each entry records the target [`PeerId`] alongside the keyed
/// `oneshot::Sender`; the sender is moved out and used exactly once on the
/// response, failure, or connection-closed path.
#[derive(Debug)]
pub(crate) struct OutboundTable<Id = OutboundRequestId> {
    pending: HashMap<Id, (PeerId, Reply)>,
}

impl<Id> Default for OutboundTable<Id> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }
}

impl<Id: Eq + Hash + Copy + std::fmt::Debug> OutboundTable<Id> {
    /// Records a pending outbound request to `peer`. libp2p guarantees fresh
    /// ids per `send_request` call so collisions are unreachable; the
    /// `debug_assert!` catches a future id-recycling regression in
    /// debug/test builds while keeping the release-build cost identical
    /// to a bare `HashMap::insert`.
    pub(crate) fn insert(&mut self, id: Id, peer: PeerId, reply: Reply) {
        let prior = self.pending.insert(id, (peer, reply));
        debug_assert!(
            prior.is_none(),
            "OutboundTable id collision: libp2p reused request id {id:?}",
        );
    }

    /// Wakes the pending caller for `id` with the libp2p response.
    /// Silently drops the result if the caller was cancelled (the
    /// oneshot receiver was dropped).
    pub(crate) fn fulfill(&mut self, id: Id, response: RpcResponse) {
        if let Some((_peer, reply)) = self.pending.remove(&id) {
            let _ = reply.send(Ok(response));
        }
    }

    /// Wakes the pending caller with an `Outbound` error. Same drop
    /// semantics as [`Self::fulfill`].
    pub(crate) fn fail(&mut self, id: Id, reason: impl Into<String>) {
        if let Some((_peer, reply)) = self.pending.remove(&id) {
            let _ = reply.send(Err(RpcError::Outbound(reason.into())));
        }
    }

    /// Fails every pending request that was sent to `peer`, waking each
    /// caller with an `Outbound` error. Called on `ConnectionClosed` so a
    /// dropped connection never strands its in-flight requests in the table
    /// for the swarm task's lifetime (closes the outbound-table leak).
    pub(crate) fn fail_all_for_peer(&mut self, peer: PeerId, reason: impl Into<String>) {
        let reason = reason.into();
        let ids: Vec<Id> = self
            .pending
            .iter()
            .filter(|(_, (p, _))| *p == peer)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((_peer, reply)) = self.pending.remove(&id) {
                let _ = reply.send(Err(RpcError::Outbound(reason.clone())));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use libp2p::PeerId;
    use tokio::sync::oneshot;

    use super::{OutboundTable, RpcError};

    // u64 keys stand in for `OutboundRequestId` (which has no public ctor).
    #[test]
    fn fail_all_for_peer_drains_only_that_peers_requests() {
        let mut table: OutboundTable<u64> = OutboundTable::default();
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let (tx1, mut rx1) = oneshot::channel();
        let (tx2, mut rx2) = oneshot::channel();
        let (tx3, mut rx3) = oneshot::channel();
        table.insert(1, peer_a, tx1);
        table.insert(2, peer_a, tx2);
        table.insert(3, peer_b, tx3);

        table.fail_all_for_peer(peer_a, "connection closed");

        // peer_a's two callers are woken with an Outbound error.
        assert!(matches!(rx1.try_recv(), Ok(Err(RpcError::Outbound(_)))));
        assert!(matches!(rx2.try_recv(), Ok(Err(RpcError::Outbound(_)))));
        // peer_b's caller is untouched — still pending (sender held by table).
        assert!(matches!(
            rx3.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn fail_all_for_peer_on_unknown_peer_is_a_noop() {
        let mut table: OutboundTable<u64> = OutboundTable::default();
        let peer = PeerId::random();
        let (tx, mut rx) = oneshot::channel();
        table.insert(1, peer, tx);

        table.fail_all_for_peer(PeerId::random(), "connection closed");

        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }
}
